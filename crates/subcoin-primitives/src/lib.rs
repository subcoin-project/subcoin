//! Primitives for the client.

use bitcoin::blockdata::block::Header as BitcoinHeader;
use bitcoin::consensus::{Decodable, Encodable};
use bitcoin::constants::genesis_block;
use bitcoin::hashes::Hash;
use bitcoin::{Block as BitcoinBlock, BlockHash, Transaction, Txid};
use codec::{Decode, Encode};
use sc_client_api::AuxStore;
use sp_blockchain::HeaderBackend;
use sp_runtime::generic::{Digest, DigestItem};
use sp_runtime::traits::{Block as BlockT, Header as HeaderT};
use std::sync::Arc;
use subcoin_runtime_primitives::{NAKAMOTO_HASH_ENGINE_ID, NAKAMOTO_HEADER_ENGINE_ID};

pub use subcoin_runtime_primitives as runtime;

type Height = u32;

/// 6 blocks is the standard confirmation period in the Bitcoin community.
pub const CONFIRMATION_DEPTH: u32 = 6u32;

/// Returns the encoded Bitcoin genesis block.
///
/// Used in the Substrate genesis block construction.
pub fn raw_genesis_tx(network: bitcoin::Network) -> Vec<u8> {
    let mut data = Vec::new();

    genesis_block(network)
        .txdata
        .into_iter()
        .next()
        .expect("Bitcoin genesis tx must exist; qed")
        .consensus_encode(&mut data)
        .expect("Genesis tx must be valid; qed");

    data
}

/// Returns the encoded Bitcoin genesis block.
pub fn bitcoin_genesis_tx() -> Vec<u8> {
    raw_genesis_tx(bitcoin::Network::Bitcoin)
}

/// Represents an indexed Bitcoin block, identified by its block number and hash.
#[derive(Debug, Clone, Copy)]
pub struct IndexedBlock {
    /// Block number.
    pub number: u32,
    /// Block hash.
    pub hash: BlockHash,
}

impl std::fmt::Display for IndexedBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{},{}", self.number, self.hash)
    }
}

impl Default for IndexedBlock {
    fn default() -> Self {
        Self {
            number: 0u32,
            hash: BlockHash::all_zeros(),
        }
    }
}

/// Trait for converting between Substrate extrinsics and Bitcoin transactions.
pub trait BitcoinTransactionAdapter<Block: BlockT> {
    /// Converts Substrate extrinsic to Bitcoin transaction.
    fn extrinsic_to_bitcoin_transaction(extrinsics: &Block::Extrinsic) -> Transaction;

    /// Converts a Bitcoin transaction into a Substrate extrinsic.
    fn bitcoin_transaction_into_extrinsic(btc_tx: Transaction) -> Block::Extrinsic;
}

/// Trait for interfacing with the Bitcoin storage.
///
/// Th essence of this trait is the mapping of the hashes between the substrate block
/// and the corresponding bitcoin block.
///
/// The mapping is stored in the client's auxiliary database.
pub trait BackendExt<Block: BlockT> {
    /// Whether the specified Bitcoin block exists in the system.
    fn block_exists(&self, bitcoin_block_hash: BlockHash) -> bool;

    /// Returns the number for given bitcoin block hash.
    ///
    /// Returns `None` if the header is not in the chain.
    fn block_number(&self, bitcoin_block_hash: BlockHash) -> Option<Height>;

    /// Returns the bitcoin block hash for given block number.
    ///
    /// Returns `None` if the header is not in the chain.
    fn block_hash(&self, block_number: u32) -> Option<BlockHash>;

    /// Returns the header for given bitcoin block hash.
    fn block_header(&self, bitcoin_block_hash: BlockHash) -> Option<BitcoinHeader>;

    /// Returns `Some(BlockHash)` if a corresponding Bitcoin block hash is found, otherwise returns `None`.
    fn bitcoin_block_hash_for(
        &self,
        substrate_block_hash: <Block as BlockT>::Hash,
    ) -> Option<BlockHash>;

    /// Returns `Some(Block::Hash)` if a corresponding Substrate block hash is found, otherwise returns `None`.
    fn substrate_block_hash_for(
        &self,
        bitcoin_block_hash: BlockHash,
    ) -> Option<<Block as BlockT>::Hash>;
}

impl<Block, Client> BackendExt<Block> for Arc<Client>
where
    Block: BlockT,
    Client: HeaderBackend<Block> + AuxStore,
{
    fn block_exists(&self, bitcoin_block_hash: BlockHash) -> bool {
        self.get_aux(bitcoin_block_hash.as_ref())
            .ok()
            .flatten()
            .is_some()
    }

    fn block_number(&self, bitcoin_block_hash: BlockHash) -> Option<Height> {
        self.substrate_block_hash_for(bitcoin_block_hash)
            .and_then(|substrate_block_hash| self.number(substrate_block_hash).ok().flatten())
            .map(|number| {
                number
                    .try_into()
                    .unwrap_or_else(|_| panic!("BlockNumber must fit into u32; qed"))
            })
    }

    fn block_hash(&self, number: u32) -> Option<BlockHash> {
        self.hash(number.into())
            .ok()
            .flatten()
            .and_then(|substrate_block_hash| self.bitcoin_block_hash_for(substrate_block_hash))
    }

    fn block_header(&self, bitcoin_block_hash: BlockHash) -> Option<BitcoinHeader> {
        self.substrate_block_hash_for(bitcoin_block_hash)
            .and_then(|substrate_block_hash| self.header(substrate_block_hash).ok().flatten())
            .and_then(|header| extract_bitcoin_block_header::<Block>(&header).ok())
    }

    fn bitcoin_block_hash_for(
        &self,
        substrate_block_hash: <Block as BlockT>::Hash,
    ) -> Option<BlockHash> {
        self.header(substrate_block_hash)
            .ok()
            .flatten()
            .and_then(|substrate_header| {
                extract_bitcoin_block_hash::<Block>(&substrate_header).ok()
            })
    }

    fn substrate_block_hash_for(
        &self,
        bitcoin_block_hash: BlockHash,
    ) -> Option<<Block as BlockT>::Hash> {
        self.get_aux(bitcoin_block_hash.as_ref())
            .map_err(|err| {
                tracing::error!(
                    ?bitcoin_block_hash,
                    "Failed to fetch substrate block hash: {err:?}"
                );
            })
            .ok()
            .flatten()
            .and_then(|substrate_hash| Decode::decode(&mut substrate_hash.as_slice()).ok())
    }
}

/// A trait to extend the Substrate Client.
pub trait ClientExt<Block> {
    /// Returns the number of best block.
    fn best_number(&self) -> u32;
}

impl<Block, Client> ClientExt<Block> for Arc<Client>
where
    Block: BlockT,
    Client: HeaderBackend<Block>,
{
    fn best_number(&self) -> u32 {
        self.info()
            .best_number
            .try_into()
            .unwrap_or_else(|_| panic!("BlockNumber must fit into u32; qed"))
    }
}

/// Deals with the storage key for UTXO in the state.
pub trait CoinStorageKey: Send + Sync {
    /// Returns the storage key for the given output specified by (txid, vout).
    fn storage_key(&self, txid: bitcoin::Txid, vout: u32) -> Vec<u8>;

    /// Returns the final storage prefix for Coins.
    fn storage_prefix(&self) -> [u8; 32];
}

/// Represents a Bitcoin block locator, used to sync blockchain data between nodes.
#[derive(Debug, Clone)]
pub struct BlockLocator {
    /// The latest block number.
    pub latest_block: u32,
    /// A vector of block hashes, starting from the latest block and going backwards.
    pub locator_hashes: Vec<BlockHash>,
}

impl BlockLocator {
    pub fn empty() -> Self {
        Self {
            latest_block: 0u32,
            locator_hashes: Vec::new(),
        }
    }
}

/// A trait for retrieving block locators.
pub trait BlockLocatorProvider<Block: BlockT> {
    /// Retrieve a block locator from given height.
    ///
    /// If `from` is None, the block locator is generated from the current best block.
    fn block_locator(
        &self,
        from: Option<Height>,
        search_pending_block: impl Fn(Height) -> Option<BlockHash>,
    ) -> BlockLocator;
}

impl<Block, Client> BlockLocatorProvider<Block> for Arc<Client>
where
    Block: BlockT,
    Client: HeaderBackend<Block> + AuxStore,
{
    fn block_locator(
        &self,
        from: Option<Height>,
        search_pending_block: impl Fn(Height) -> Option<BlockHash>,
    ) -> BlockLocator {
        let mut locator_hashes = Vec::new();

        let from = from.unwrap_or_else(|| self.best_number());

        for height in locator_indexes(from) {
            // if height < last_checkpoint {
            // Don't go past the latest checkpoint. We never want to accept a fork
            // older than our last checkpoint.
            // break;
            // }

            if let Some(bitcoin_hash) = search_pending_block(height) {
                locator_hashes.push(bitcoin_hash);
                continue;
            }

            let Ok(Some(hash)) = self.hash(height.into()) else {
                continue;
            };

            if let Ok(Some(header)) = self.header(hash) {
                let maybe_bitcoin_block_hash =
                    BackendExt::<Block>::bitcoin_block_hash_for(self, header.hash());
                if let Some(bitcoin_block_hash) = maybe_bitcoin_block_hash {
                    locator_hashes.push(bitcoin_block_hash);
                }
            }
        }

        BlockLocator {
            latest_block: from,
            locator_hashes,
        }
    }
}

/// Get the locator indexes starting from a given height, and going backwards, exponentially
/// backing off.
fn locator_indexes(mut from: Height) -> Vec<Height> {
    let mut indexes = Vec::new();
    let mut step = 1;

    while from > 0 {
        // For the first 8 blocks, don't skip any heights.
        if indexes.len() >= 8 {
            step *= 2;
        }
        indexes.push(from as Height);
        from = from.saturating_sub(step);
    }

    // Always include genesis.
    indexes.push(0);

    indexes
}

/// Represents the index of a transaction.
#[derive(Debug, Clone, Encode, Decode)]
pub struct TxPosition {
    /// Number of the block including the transaction.
    pub block_number: u32,
    /// Position of the transaction within the block.
    pub index: u32,
}

/// Interface for retriving the position of given transaction ID.
pub trait TransactionIndex {
    /// Returns the position of given transaction ID if any.
    fn tx_index(&self, txid: Txid) -> sp_blockchain::Result<Option<TxPosition>>;
}

/// Dummy implementor of [`TransactionIndex`].
pub struct NoTransactionIndex;

impl TransactionIndex for NoTransactionIndex {
    fn tx_index(&self, _txid: Txid) -> sp_blockchain::Result<Option<TxPosition>> {
        Ok(None)
    }
}

/// Constructs a Substrate header digest from a Bitcoin header.
///
/// NOTE: The bitcoin block hash digest is stored in the reversed byte order, making it
/// user-friendly on polkadot.js.org.
pub fn substrate_header_digest(bitcoin_header: &BitcoinHeader) -> Digest {
    let mut raw_bitcoin_block_hash = bitcoin_header.block_hash().to_byte_array().to_vec();
    raw_bitcoin_block_hash.reverse();

    let mut encoded_bitcoin_header = Vec::with_capacity(32);
    bitcoin_header
        .consensus_encode(&mut encoded_bitcoin_header)
        .expect("Bitcoin header must be valid; qed");

    // Store the Bitcoin block hash and the bitcoin header itself in the header digest.
    //
    // Storing the Bitcoin block hash redundantly is used to retrieve it quickly without
    // decoding the entire bitcoin header later.
    Digest {
        logs: vec![
            DigestItem::PreRuntime(NAKAMOTO_HASH_ENGINE_ID, raw_bitcoin_block_hash),
            DigestItem::PreRuntime(NAKAMOTO_HEADER_ENGINE_ID, encoded_bitcoin_header),
        ],
    }
}

/// Error type of Subcoin header.
#[derive(Debug, Clone)]
pub enum HeaderError {
    MultiplePreRuntimeDigests,
    MissingBitcoinBlockHashDigest,
    InvalidBitcoinBlockHashDigest,
    MissingBitcoinBlockHeader,
    InvalidBitcoinBlockHeader(String),
}

/// Extracts the Bitcoin block hash from the given Substrate header.
pub fn extract_bitcoin_block_hash<Block: BlockT>(
    header: &Block::Header,
) -> Result<BlockHash, HeaderError> {
    let mut pre_digest: Option<_> = None;

    for log in header.digest().logs() {
        tracing::trace!("Checking log {:?}, looking for pre runtime digest", log);
        match (log, pre_digest.is_some()) {
            (DigestItem::PreRuntime(NAKAMOTO_HASH_ENGINE_ID, _), true) => {
                return Err(HeaderError::MultiplePreRuntimeDigests);
            }
            (DigestItem::PreRuntime(NAKAMOTO_HASH_ENGINE_ID, v), false) => {
                pre_digest.replace(v);
            }
            (_, _) => tracing::trace!("Ignoring digest not meant for us"),
        }
    }

    let mut raw_bitcoin_block_hash = pre_digest
        .ok_or(HeaderError::MissingBitcoinBlockHashDigest)?
        .to_vec();
    raw_bitcoin_block_hash.reverse();

    BlockHash::from_slice(&raw_bitcoin_block_hash)
        .map_err(|_| HeaderError::InvalidBitcoinBlockHashDigest)
}

/// Extracts the Bitcoin block header from the given Substrate header.
pub fn extract_bitcoin_block_header<Block: BlockT>(
    header: &Block::Header,
) -> Result<BitcoinHeader, HeaderError> {
    let mut pre_digest: Option<_> = None;

    for log in header.digest().logs() {
        tracing::trace!("Checking log {:?}, looking for pre runtime digest", log);
        match (log, pre_digest.is_some()) {
            (DigestItem::PreRuntime(NAKAMOTO_HEADER_ENGINE_ID, _), true) => {
                return Err(HeaderError::MultiplePreRuntimeDigests);
            }
            (DigestItem::PreRuntime(NAKAMOTO_HEADER_ENGINE_ID, v), false) => {
                pre_digest.replace(v);
            }
            (_, _) => tracing::trace!("Ignoring digest not meant for us"),
        }
    }

    let bitcoin_block_header = pre_digest.ok_or(HeaderError::MissingBitcoinBlockHeader)?;

    BitcoinHeader::consensus_decode(&mut bitcoin_block_header.as_slice())
        .map_err(|err| HeaderError::InvalidBitcoinBlockHeader(err.to_string()))
}

/// Converts a Substrate block to a Bitcoin block.
pub fn convert_to_bitcoin_block<
    Block: BlockT,
    TransactionAdapter: BitcoinTransactionAdapter<Block>,
>(
    substrate_block: Block,
) -> Result<BitcoinBlock, HeaderError> {
    let header = extract_bitcoin_block_header::<Block>(substrate_block.header())?;

    let txdata = substrate_block
        .extrinsics()
        .iter()
        .map(TransactionAdapter::extrinsic_to_bitcoin_transaction)
        .collect();

    Ok(BitcoinBlock { header, txdata })
}
