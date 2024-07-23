mod header_verify;
mod tx_verify;

use crate::chain_params::ChainParams;
use bitcoin::block::Bip34Error;
use bitcoin::blockdata::block::Header as BitcoinHeader;
use bitcoin::blockdata::constants::MAX_BLOCK_SIGOPS_COST;
use bitcoin::blockdata::weight::WITNESS_SCALE_FACTOR;
use bitcoin::consensus::Encodable;
use bitcoin::{
    Amount, Block as BitcoinBlock, BlockHash, OutPoint, ScriptBuf, Transaction, TxMerkleNode,
    TxOut, Txid, VarInt, Weight,
};
use sc_client_api::{AuxStore, Backend, StorageProvider};
use sp_blockchain::HeaderBackend;
use sp_runtime::traits::Block as BlockT;
use std::collections::{HashMap, HashSet};
use std::ffi::c_uint;
use std::marker::PhantomData;
use std::sync::Arc;
use subcoin_primitives::runtime::{bitcoin_block_subsidy, Coin};
use subcoin_primitives::CoinStorageKey;
use tx_verify::{check_transaction_sanity, is_final};

pub use header_verify::{Error as HeaderError, HeaderVerifier};
pub use tx_verify::Error as TxError;

/// The maximum allowed weight for a block, see BIP 141 (network rule).
pub const MAX_BLOCK_WEIGHT: Weight = Weight::MAX_BLOCK;

/// Represents the level of block verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "cli", derive(clap::ValueEnum))]
pub enum BlockVerification {
    /// No verification performed.
    None,
    /// Full verification, including verifying the transactions.
    Full,
    /// Verify the block header only, without the transaction veification.
    HeaderOnly,
}

/// Block verification error.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The merkle root of the block is invalid.
    #[error("Invalid merkle root")]
    BadMerkleRoot,
    /// Block must contain at least one coinbase transaction.
    #[error("Transaction list is empty")]
    EmptyTransactionList,
    #[error("Block is too large")]
    BadBlockLength,
    #[error("First transaction is not coinbase")]
    FirstTransactionIsNotCoinbase,
    #[error("Block contains more than one coinbase")]
    MultipleCoinbase,
    #[error("Transaction input script contains too many sigops (max: {MAX_BLOCK_SIGOPS_COST})")]
    TooManySigOps { block_number: u32 },
    #[error("Invalid witness commitment")]
    BadWitnessCommitment,
    #[error("Transaction is not finalized")]
    TransactionNotFinal,
    #[error("Block contains duplicate transaction at index {0}")]
    DuplicateTransaction(usize),
    #[error("Block height mismatches in coinbase (got: {got}, expected: {expected})")]
    BadCoinbaseBlockHeight { got: u32, expected: u32 },
    /// Referenced output does not exist or was spent before.
    #[error("UTXO not found (#{block_number}:{txid}: {out_point:?})")]
    UtxoNotFound {
        block_number: u32,
        txid: Txid,
        out_point: OutPoint,
    },
    /// Referenced output has already been spent in this block.
    #[error("UTXO already spent in current block (#{block_number}:{txid}: {out_point:?})")]
    AlreadySpentInCurrentBlock {
        block_number: u32,
        txid: Txid,
        out_point: OutPoint,
    },
    #[error("Total output amount exceeds total input amount")]
    InsufficientFunds,
    // Invalid coinbase value.
    #[error("Block reward is larger than the sum of block fee and subsidy")]
    InvalidBlockReward,
    #[error(transparent)]
    Transaction(#[from] TxError),
    /// Block header error.
    #[error(transparent)]
    Header(#[from] HeaderError),
    #[error(transparent)]
    BitcoinConsensus(#[from] bitcoin::consensus::validation::BitcoinconsensusError),
    #[error(transparent)]
    Bip34(#[from] Bip34Error),
    #[error("Bitcoin codec: {0:?}")]
    BitcoinCodec(bitcoin::io::Error),
    /// An error occurred in the client.
    #[error(transparent)]
    Client(#[from] sp_blockchain::Error),
}

/// A struct responsible for verifying Bitcoin blocks.
#[derive(Clone)]
pub struct BlockVerifier<Block, Client, BE> {
    client: Arc<Client>,
    chain_params: ChainParams,
    header_verifier: HeaderVerifier<Block, Client>,
    block_verification: BlockVerification,
    coin_storage_key: Arc<dyn CoinStorageKey>,
    _phantom: PhantomData<(Block, BE)>,
}

impl<Block, Client, BE> BlockVerifier<Block, Client, BE> {
    /// Constructs a new instance of [`BlockVerifier`].
    pub fn new(
        client: Arc<Client>,
        network: bitcoin::Network,
        block_verification: BlockVerification,
        coin_storage_key: Arc<dyn CoinStorageKey>,
    ) -> Self {
        let chain_params = ChainParams::new(network);
        let header_verifier = HeaderVerifier::new(client.clone(), chain_params.clone());
        Self {
            client,
            chain_params,
            header_verifier,
            block_verification,
            coin_storage_key,
            _phantom: Default::default(),
        }
    }
}

impl<Block, Client, BE> BlockVerifier<Block, Client, BE>
where
    Block: BlockT,
    BE: Backend<Block>,
    Client: HeaderBackend<Block> + StorageProvider<Block, BE> + AuxStore,
{
    /// Performs full block verification.
    ///
    /// References:
    /// - <https://en.bitcoin.it/wiki/Protocol_rules#.22block.22_messages>
    pub fn verify_block(&self, block_number: u32, block: &BitcoinBlock) -> Result<(), Error> {
        let txids = self.check_block_sanity(block_number, block)?;

        self.contextual_check_block(block_number, block, txids)
    }

    fn contextual_check_block(
        &self,
        block_number: u32,
        block: &BitcoinBlock,
        txids: HashMap<usize, Txid>,
    ) -> Result<(), Error> {
        match self.block_verification {
            BlockVerification::Full => {
                let lock_time_cutoff = self.header_verifier.verify_header(&block.header)?;

                if block_number >= self.chain_params.segwit_height
                    && !block.check_witness_commitment()
                {
                    return Err(Error::BadWitnessCommitment);
                }

                // Check the block weight with witness data.
                if block.weight() > MAX_BLOCK_WEIGHT {
                    return Err(Error::BadBlockLength);
                }

                self.verify_transactions(block_number, block, txids, lock_time_cutoff)?;
            }
            BlockVerification::HeaderOnly => {
                self.header_verifier.verify_header(&block.header)?;
            }
            BlockVerification::None => {}
        }

        Ok(())
    }

    /// Performs preliminary checks.
    ///
    /// - Transaction list must be non-empty.
    /// - Block size must not exceed [`MAX_BLOCK_WEIGHT`].
    /// - First transaction must be coinbase, the rest must not be.
    /// - No duplicate transactions in the block.
    /// - Check the sum of transaction sig opcounts does not exceed [`MAX_BLOCK_SIGOPS_COST`].
    /// - Check the calculated merkle root of transactions matches the one declared in the header.
    ///
    /// <https://github.com/bitcoin/bitcoin/blob/6f9db1ebcab4064065ccd787161bf2b87e03cc1f/src/validation.cpp#L3986>
    fn check_block_sanity(
        &self,
        block_number: u32,
        block: &BitcoinBlock,
    ) -> Result<HashMap<usize, Txid>, Error> {
        if block.txdata.is_empty() {
            return Err(Error::EmptyTransactionList);
        }

        // Size limits, without tx witness data.
        if Weight::from_wu(block.txdata.len() as u64 * WITNESS_SCALE_FACTOR as u64)
            > MAX_BLOCK_WEIGHT
            || Weight::from_wu(block_base_size(block) as u64 * WITNESS_SCALE_FACTOR as u64)
                > MAX_BLOCK_WEIGHT
        {
            return Err(Error::BadBlockLength);
        }

        if !block.txdata[0].is_coinbase() {
            return Err(Error::FirstTransactionIsNotCoinbase);
        }

        let get_legacy_sig_op_count = |tx: &Transaction| {
            let mut sig_ops = 0;

            tx.input.iter().for_each(|txin| {
                sig_ops += txin.script_sig.count_sigops_legacy();
            });

            tx.output.iter().for_each(|txout| {
                sig_ops += txout.script_pubkey.count_sigops_legacy();
            });

            sig_ops
        };

        // Check duplicate transactions
        let tx_count = block.txdata.len();

        let mut seen_transactions = HashSet::with_capacity(tx_count);
        let mut txids = HashMap::with_capacity(tx_count);

        let mut sig_ops = 0;

        for (index, tx) in block.txdata.iter().enumerate() {
            if index > 0 && tx.is_coinbase() {
                return Err(Error::MultipleCoinbase);
            }

            let txid = tx.compute_txid();
            if !seen_transactions.insert(txid) {
                // If txid is already in the set, we've found a duplicate.
                return Err(Error::DuplicateTransaction(index));
            }

            check_transaction_sanity(tx)?;

            sig_ops += get_legacy_sig_op_count(tx);

            txids.insert(index, txid);
        }

        if sig_ops * WITNESS_SCALE_FACTOR > MAX_BLOCK_SIGOPS_COST as usize {
            return Err(Error::TooManySigOps { block_number });
        }

        // Inline `Block::check_merkle_root()` to avoid redundantly computing txid.
        let hashes = block
            .txdata
            .iter()
            .enumerate()
            .filter_map(|(index, _obj)| txids.get(&index).map(|txid| txid.to_raw_hash()));

        let maybe_merkle_root: Option<TxMerkleNode> =
            bitcoin::merkle_tree::calculate_root(hashes).map(|h| h.into());

        if !maybe_merkle_root
            .map(|merkle_root| block.header.merkle_root == merkle_root)
            .unwrap_or(false)
        {
            return Err(Error::BadMerkleRoot);
        }

        Ok(txids)
    }

    fn verify_transactions(
        &self,
        block_number: u32,
        block: &BitcoinBlock,
        txids: HashMap<usize, Txid>,
        lock_time_cutoff: u32,
    ) -> Result<(), Error> {
        let parent_number = block_number - 1;
        let parent_hash =
            self.client
                .hash(parent_number.into())?
                .ok_or(sp_blockchain::Error::Backend(format!(
                    "Parent block #{parent_number} not found"
                )))?;

        let get_txid = |tx_index: usize| {
            txids
                .get(&tx_index)
                .copied()
                .expect("Txid must exist as initialized in `check_block_sanity()`; qed")
        };

        let flags = get_block_script_flags(block_number, block.block_hash(), &self.chain_params);

        let mut block_fee = 0;
        let mut spent_utxos = HashSet::new();

        let mut tx_data = Vec::<u8>::new();

        // TODO: verify transactions in parallel.
        for (tx_index, tx) in block.txdata.iter().enumerate() {
            if tx_index == 0 {
                // Enforce rule that the coinbase starts with serialized block height.
                if block_number >= self.chain_params.params.bip34_height {
                    let block_height_in_coinbase = block.bip34_block_height()? as u32;
                    if block_height_in_coinbase != block_number {
                        return Err(Error::BadCoinbaseBlockHeight {
                            got: block_height_in_coinbase,
                            expected: block_number,
                        });
                    }
                }

                continue;
            }

            if !is_final(tx, block_number, lock_time_cutoff) {
                return Err(Error::TransactionNotFinal);
            }

            tx_data.clear();
            tx.consensus_encode(&mut tx_data)
                .map_err(Error::BitcoinCodec)?;

            let spending_transaction = tx_data.as_slice();

            let mut total_input_value = 0;

            for (input_index, input) in tx.input.iter().enumerate() {
                let out_point = input.previous_output;

                if spent_utxos.contains(&out_point) {
                    return Err(Error::AlreadySpentInCurrentBlock {
                        block_number,
                        txid: get_txid(tx_index),
                        out_point,
                    });
                }

                let spent_output = match self.find_utxo_in_state(parent_hash, out_point) {
                    Some(coin) => TxOut {
                        value: Amount::from_sat(coin.amount),
                        script_pubkey: ScriptBuf::from_bytes(coin.script_pubkey),
                    },
                    None => find_utxo_in_current_block(block, out_point, tx_index, get_txid)
                        .ok_or_else(|| Error::UtxoNotFound {
                            block_number,
                            txid: get_txid(tx_index),
                            out_point,
                        })?,
                };

                bitcoin::consensus::validation::verify_script_with_flags(
                    &spent_output.script_pubkey,
                    input_index,
                    spent_output.value,
                    spending_transaction,
                    flags,
                )?;

                spent_utxos.insert(out_point);
                total_input_value += spent_output.value.to_sat();
            }

            let total_output_value = tx
                .output
                .iter()
                .map(|output| output.value.to_sat())
                .sum::<u64>();

            // Total input value must be no less than total output value.
            // Tx fee is the difference between inputs and outputs.
            let tx_fee = total_input_value
                .checked_sub(total_output_value)
                .ok_or(Error::InsufficientFunds)?;

            block_fee += tx_fee;
        }

        let coinbase_value = block.txdata[0]
            .output
            .iter()
            .map(|output| output.value.to_sat())
            .sum::<u64>();

        let subsidy = bitcoin_block_subsidy(block_number);

        // Ensures no inflation.
        if coinbase_value > block_fee + subsidy {
            return Err(Error::InvalidBlockReward);
        }

        Ok(())
    }

    /// Finds a UTXO in the state backend.
    fn find_utxo_in_state(&self, block_hash: Block::Hash, out_point: OutPoint) -> Option<Coin> {
        use codec::Decode;

        // Read state from the backend
        //
        // TODO: optimizations:
        // - Read the state from the in memory backend.
        // - Maintain a flat in-memory UTXO cache and try to read from cache first.
        let OutPoint { txid, vout } = out_point;
        let storage_key = self.coin_storage_key.storage_key(txid, vout);

        let maybe_storage_data = self
            .client
            .storage(block_hash, &sc_client_api::StorageKey(storage_key))
            .ok()
            .flatten();

        maybe_storage_data.and_then(|data| Coin::decode(&mut data.0.as_slice()).ok())
    }
}

// Find a UTXO from the previous transactions in current block.
fn find_utxo_in_current_block(
    block: &BitcoinBlock,
    out_point: OutPoint,
    tx_index: usize,
    get_txid: impl Fn(usize) -> Txid,
) -> Option<TxOut> {
    let OutPoint { txid, vout } = out_point;
    block
        .txdata
        .iter()
        .take(tx_index)
        .enumerate()
        .find_map(|(index, tx)| (get_txid(index) == txid).then_some(tx))
        .and_then(|tx| tx.output.get(vout as usize))
        .cloned()
}

/// Returns the script validation flags for the specified block.
fn get_block_script_flags(
    height: u32,
    block_hash: BlockHash,
    chain_params: &ChainParams,
) -> c_uint {
    if let Some(flag) = chain_params
        .script_flag_exceptions
        .get(&block_hash)
        .copied()
    {
        return flag;
    }

    let mut flags = bitcoinconsensus::VERIFY_P2SH | bitcoinconsensus::VERIFY_WITNESS;

    if height >= chain_params.params.bip65_height {
        flags |= bitcoinconsensus::VERIFY_CHECKLOCKTIMEVERIFY;
    }

    if height >= chain_params.params.bip66_height {
        flags |= bitcoinconsensus::VERIFY_DERSIG;
    }

    if height >= chain_params.csv_height {
        flags |= bitcoinconsensus::VERIFY_CHECKSEQUENCEVERIFY;
    }

    if height >= chain_params.segwit_height {
        flags |= bitcoinconsensus::VERIFY_NULLDUMMY;
    }

    flags
}

/// Returns the base block size.
///
/// > Base size is the block size in bytes with the original transaction serialization without
/// > any witness-related data, as seen by a non-upgraded node.
// TODO: copied from rust-bitcoin, send a patch upstream to make this API public?
fn block_base_size(block: &BitcoinBlock) -> usize {
    let mut size = BitcoinHeader::SIZE;

    size += VarInt::from(block.txdata.len()).size();
    size += block.txdata.iter().map(|tx| tx.base_size()).sum::<usize>();

    size
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::consensus::encode::deserialize_hex;

    #[test]
    fn test_find_utxo_in_current_block() {
        let test_block = std::env::current_dir()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("test_data")
            .join("btc_mainnet_385044.data");
        let raw_block = std::fs::read_to_string(test_block).unwrap();
        let block = deserialize_hex::<BitcoinBlock>(raw_block.trim()).unwrap();

        let txids = block
            .txdata
            .iter()
            .enumerate()
            .map(|(index, tx)| (index, tx.compute_txid()))
            .collect::<HashMap<_, _>>();

        // 385044:35:1
        let out_point = OutPoint {
            txid: "2b102a19161e5c93f71e16f9e8c9b2438f362c51ecc8f2a62e3c31d7615dd17d"
                .parse()
                .unwrap(),
            vout: 1,
        };

        // The input of block 385044:36 is from the previous transaction 385044:35:1.
        // https://www.blockchain.com/explorer/transactions/btc/5645cb0a3953b7766836919566b25321a976d06c958e69ff270358233a8c82d6
        assert_eq!(
            find_utxo_in_current_block(&block, out_point, 36, |index| txids
                .get(&index)
                .copied()
                .unwrap())
            .map(|txout| txout.value.to_sat()),
            Some(295600000)
        );
    }
}
