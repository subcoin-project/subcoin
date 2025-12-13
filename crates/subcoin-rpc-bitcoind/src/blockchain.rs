//! Bitcoin Core compatible blockchain RPC methods.
//!
//! Implements: getblock, getblockhash, getblockheader, getblockchaininfo, getbestblockhash

use crate::error::Error;
use crate::types::{GetBlock, GetBlockHeader, GetBlockchainInfo};
use bitcoin::consensus::Encodable;
use bitcoin::{Block as BitcoinBlock, BlockHash};
use jsonrpsee::proc_macros::rpc;
use sc_client_api::{AuxStore, BlockBackend, HeaderBackend};
use sp_runtime::traits::{Block as BlockT, SaturatedConversion};
use std::marker::PhantomData;
use std::sync::Arc;
use subcoin_primitives::{BackendExt, BitcoinTransactionAdapter, convert_to_bitcoin_block};

/// Bitcoin Core compatible blockchain RPC API.
#[rpc(client, server)]
pub trait BlockchainApi {
    /// Returns the hash of the best (tip) block in the most-work fully-validated chain.
    #[method(name = "getbestblockhash", blocking)]
    fn get_best_block_hash(&self) -> Result<BlockHash, Error>;

    /// Returns an object containing various state info regarding blockchain processing.
    #[method(name = "getblockchaininfo", blocking)]
    fn get_blockchain_info(&self) -> Result<GetBlockchainInfo, Error>;

    /// Returns the height of the most-work fully-validated chain.
    #[method(name = "getblockcount", blocking)]
    fn get_block_count(&self) -> Result<u32, Error>;

    /// Returns hash of block in best-block-chain at height provided.
    #[method(name = "getblockhash", blocking)]
    fn get_block_hash(&self, height: u32) -> Result<BlockHash, Error>;

    /// Returns block data.
    ///
    /// If verbosity is 0, returns a hex-encoded serialized block.
    /// If verbosity is 1 (default), returns a JSON object with block info and txids.
    /// If verbosity is 2, returns a JSON object with block info and full tx data.
    #[method(name = "getblock", blocking)]
    fn get_block(
        &self,
        blockhash: BlockHash,
        verbosity: Option<u8>,
    ) -> Result<serde_json::Value, Error>;

    /// Returns information about a block header.
    ///
    /// If verbose is false (default=true), returns a hex-encoded serialized header.
    /// If verbose is true, returns a JSON object with header info.
    #[method(name = "getblockheader", blocking)]
    fn get_block_header(
        &self,
        blockhash: BlockHash,
        verbose: Option<bool>,
    ) -> Result<serde_json::Value, Error>;
}

/// Calculate difficulty from compact bits representation.
fn difficulty_from_bits(bits: u32) -> f64 {
    // Bitcoin difficulty calculation from compact bits
    // Difficulty 1 target is 0x00000000FFFF... (256-bit), but we use a simplified formula
    let mantissa = bits & 0x00ff_ffff;
    let exponent = (bits >> 24) as i32;

    if mantissa == 0 {
        return 0.0;
    }

    // difficulty = difficulty_1_target / current_target
    // Using the simplified formula from Bitcoin Core
    let shift = 8 * (exponent - 3);
    let diff = (0x0000ffff_u64 as f64) / (mantissa as f64) * 2f64.powi(-shift);
    if diff > 0.0 { diff } else { 0.0 }
}

/// Bitcoin Core compatible blockchain RPC implementation.
pub struct Blockchain<Block, Client, TransactionAdapter> {
    client: Arc<Client>,
    network: bitcoin::Network,
    _phantom: PhantomData<(Block, TransactionAdapter)>,
}

impl<Block, Client, TransactionAdapter> Blockchain<Block, Client, TransactionAdapter>
where
    Block: BlockT + 'static,
    Client: HeaderBackend<Block> + BlockBackend<Block> + AuxStore + 'static,
{
    /// Creates a new instance of [`Blockchain`].
    pub fn new(client: Arc<Client>, network: bitcoin::Network) -> Self {
        Self {
            client,
            network,
            _phantom: Default::default(),
        }
    }

    fn get_bitcoin_block(&self, block_hash: BlockHash) -> Result<BitcoinBlock, Error>
    where
        TransactionAdapter: BitcoinTransactionAdapter<Block>,
    {
        let substrate_block_hash = self
            .client
            .substrate_block_hash_for(block_hash)
            .ok_or(Error::BlockNotFound)?;

        let substrate_block = self
            .client
            .block(substrate_block_hash)?
            .ok_or(Error::BlockNotFound)?
            .block;

        convert_to_bitcoin_block::<Block, TransactionAdapter>(substrate_block)
            .map_err(Error::Header)
    }

    fn get_block_number(&self, block_hash: BlockHash) -> Result<u32, Error> {
        self.client
            .block_number(block_hash)
            .ok_or(Error::BlockNotFound)
    }

    fn best_number(&self) -> u32 {
        self.client.info().best_number.saturated_into()
    }

    fn calculate_confirmations(&self, block_height: u32) -> i32 {
        let best = self.best_number();
        if block_height > best {
            0
        } else {
            (best - block_height + 1) as i32
        }
    }

    fn get_next_block_hash(&self, block_height: u32) -> Option<BlockHash> {
        self.client.block_hash(block_height + 1)
    }

    fn block_to_get_block(&self, block: &BitcoinBlock, height: u32) -> GetBlock {
        let header = &block.header;
        let block_hash = header.block_hash();
        let txids = block.txdata.iter().map(|tx| tx.compute_txid()).collect();

        // Calculate block size and weight
        let mut size_data = Vec::new();
        block
            .consensus_encode(&mut size_data)
            .expect("encoding should not fail");
        let size = size_data.len() as u32;

        // Stripped size (without witness) - calculate from weight
        let weight = block.weight().to_wu() as u32;
        // weight = base_size * 3 + total_size, so base_size = (weight - size) / 3 + (size - size) ... simplified
        // Actually: weight = (size - witness_size) * 4 + witness_size = size * 4 - witness_size * 3
        // So stripped_size = (4 * size - weight) / 3
        let strippedsize = (4 * size).saturating_sub(weight) / 3;

        GetBlock {
            hash: block_hash,
            confirmations: self.calculate_confirmations(height),
            size,
            strippedsize,
            weight,
            height,
            version: header.version.to_consensus(),
            version_hex: format!("{:08x}", header.version.to_consensus()),
            merkleroot: header.merkle_root.to_string(),
            tx: txids,
            time: header.time,
            mediantime: header.time, // TODO: Calculate actual median time
            nonce: header.nonce,
            bits: format!("{:08x}", header.bits.to_consensus()),
            difficulty: difficulty_from_bits(header.bits.to_consensus()),
            chainwork: "0".to_string(), // TODO: Calculate chainwork
            n_tx: block.txdata.len() as u32,
            previousblockhash: if height > 0 {
                Some(header.prev_blockhash)
            } else {
                None
            },
            nextblockhash: self.get_next_block_hash(height),
        }
    }

    fn header_to_get_block_header(
        &self,
        header: &bitcoin::block::Header,
        height: u32,
        n_tx: u32,
    ) -> GetBlockHeader {
        let block_hash = header.block_hash();

        GetBlockHeader {
            hash: block_hash,
            confirmations: self.calculate_confirmations(height),
            height,
            version: header.version.to_consensus(),
            version_hex: format!("{:08x}", header.version.to_consensus()),
            merkleroot: header.merkle_root.to_string(),
            time: header.time,
            mediantime: header.time, // TODO: Calculate actual median time
            nonce: header.nonce,
            bits: format!("{:08x}", header.bits.to_consensus()),
            difficulty: difficulty_from_bits(header.bits.to_consensus()),
            chainwork: "0".to_string(), // TODO: Calculate chainwork
            n_tx,
            previousblockhash: if height > 0 {
                Some(header.prev_blockhash)
            } else {
                None
            },
            nextblockhash: self.get_next_block_hash(height),
        }
    }
}

#[async_trait::async_trait]
impl<Block, Client, TransactionAdapter> BlockchainApiServer
    for Blockchain<Block, Client, TransactionAdapter>
where
    Block: BlockT + 'static,
    Client: HeaderBackend<Block> + BlockBackend<Block> + AuxStore + 'static,
    TransactionAdapter: BitcoinTransactionAdapter<Block> + Send + Sync + 'static,
{
    fn get_best_block_hash(&self) -> Result<BlockHash, Error> {
        let best_substrate_hash = self.client.info().best_hash;
        self.client
            .bitcoin_block_hash_for(best_substrate_hash)
            .ok_or_else(|| Error::Other("Best block hash not found".to_string()))
    }

    fn get_blockchain_info(&self) -> Result<GetBlockchainInfo, Error> {
        let best_hash = self.get_best_block_hash()?;
        let best_number = self.best_number();

        // Get difficulty from best block header
        let block = self.get_bitcoin_block(best_hash)?;
        let difficulty = difficulty_from_bits(block.header.bits.to_consensus());

        Ok(GetBlockchainInfo {
            chain: self.network.to_core_arg().to_string(),
            blocks: best_number,
            headers: best_number, // Same as blocks for full node
            bestblockhash: best_hash,
            difficulty,
            verificationprogress: 1.0, // Fully synced
            initialblockdownload: false,
            chainwork: "0".to_string(), // TODO: Calculate chainwork
            size_on_disk: 0,            // TODO: Get actual disk usage
            pruned: false,
            warnings: vec![],
        })
    }

    fn get_block_count(&self) -> Result<u32, Error> {
        Ok(self.best_number())
    }

    fn get_block_hash(&self, height: u32) -> Result<BlockHash, Error> {
        self.client
            .block_hash(height)
            .ok_or(Error::BlockHeightOutOfRange)
    }

    fn get_block(
        &self,
        blockhash: BlockHash,
        verbosity: Option<u8>,
    ) -> Result<serde_json::Value, Error> {
        let block = self.get_bitcoin_block(blockhash)?;
        let height = self.get_block_number(blockhash)?;

        match verbosity.unwrap_or(1) {
            0 => {
                // Return hex-encoded serialized block
                let mut data = Vec::new();
                block
                    .consensus_encode(&mut data)
                    .map_err(|e| Error::Other(format!("Failed to encode block: {e}")))?;
                Ok(serde_json::Value::String(hex::encode(data)))
            }
            1 => {
                // Return JSON with txids
                let get_block = self.block_to_get_block(&block, height);
                Ok(serde_json::to_value(get_block)?)
            }
            2 => {
                // Return JSON with full transaction data
                // TODO: Implement full transaction decoding
                let get_block = self.block_to_get_block(&block, height);
                Ok(serde_json::to_value(get_block)?)
            }
            _ => Err(Error::Other("Invalid verbosity level".to_string())),
        }
    }

    fn get_block_header(
        &self,
        blockhash: BlockHash,
        verbose: Option<bool>,
    ) -> Result<serde_json::Value, Error> {
        let block = self.get_bitcoin_block(blockhash)?;
        let height = self.get_block_number(blockhash)?;

        if verbose.unwrap_or(true) {
            // Return JSON object
            let header_info =
                self.header_to_get_block_header(&block.header, height, block.txdata.len() as u32);
            Ok(serde_json::to_value(header_info)?)
        } else {
            // Return hex-encoded serialized header
            let mut data = Vec::new();
            block
                .header
                .consensus_encode(&mut data)
                .map_err(|e| Error::Other(format!("Failed to encode header: {e}")))?;
            Ok(serde_json::Value::String(hex::encode(data)))
        }
    }
}
