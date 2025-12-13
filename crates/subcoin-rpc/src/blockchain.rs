use crate::error::Error;
use bitcoin::blockdata::block::Header as BitcoinHeader;
use bitcoin::consensus::Encodable;
use bitcoin::{Address, Block as BitcoinBlock, BlockHash, Network, ScriptBuf, Transaction, Txid};
use jsonrpsee::proc_macros::rpc;
use sc_client_api::{AuxStore, BlockBackend, HeaderBackend};
use serde::{Deserialize, Serialize};
use sp_runtime::traits::Block as BlockT;
use std::marker::PhantomData;
use std::sync::Arc;
use subcoin_primitives::{
    BackendExt, BitcoinTransactionAdapter, TransactionIndex, TxPosition, convert_to_bitcoin_block,
    extract_bitcoin_block_header,
};
use subcoin_script::{TxoutType, solve};

/// Block data with transaction IDs included for easier client-side processing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockWithTxids {
    /// Block header.
    pub header: BitcoinHeader,
    /// List of transaction IDs in the block.
    pub txids: Vec<Txid>,
    /// Full transaction data.
    pub txdata: Vec<Transaction>,
}

/// Bitcoin blockchain API.
#[rpc(client, server)]
pub trait BlockchainApi {
    /// Get a block header by its hash.
    #[method(name = "blockchain_getHeader", blocking)]
    fn header(&self, hash: Option<BlockHash>) -> Result<Option<BitcoinHeader>, Error>;

    /// Get a full block (including the header and transactions) by its hash.
    #[method(name = "blockchain_getBlock", blocking)]
    fn block(&self, hash: Option<BlockHash>) -> Result<Option<BitcoinBlock>, Error>;

    /// Get a full block by its number.
    #[method(name = "blockchain_getBlockByNumber", blocking)]
    fn block_by_number(&self, number: Option<u32>) -> Result<Option<BitcoinBlock>, Error>;

    /// Get a full block with transaction IDs by its hash.
    #[method(name = "blockchain_getBlockWithTxids", blocking)]
    fn block_with_txids(&self, hash: Option<BlockHash>) -> Result<Option<BlockWithTxids>, Error>;

    /// Get a full block with transaction IDs by its number.
    #[method(name = "blockchain_getBlockWithTxidsByNumber", blocking)]
    fn block_with_txids_by_number(
        &self,
        number: Option<u32>,
    ) -> Result<Option<BlockWithTxids>, Error>;

    /// Get a raw block in hex format by its hash.
    #[method(name = "blockchain_getRawBlock", blocking)]
    fn raw_block(&self, hash: Option<BlockHash>) -> Result<Option<String>, Error>;

    /// Get a raw block in hex format by its number.
    #[method(name = "blockchain_getRawBlockByNumber", blocking)]
    fn raw_block_by_number(&self, number: Option<u32>) -> Result<Option<String>, Error>;

    /// Get transaction.
    #[method(name = "blockchain_getTransaction", blocking)]
    fn transaction(&self, txid: Txid) -> Result<Option<Transaction>, Error>;

    // Get current best block hash
    #[method(name = "blockchain_getBestBlockHash", blocking)]
    fn best_block_hash(&self) -> Result<BlockHash, Error>;

    // Get block hash by height
    #[method(name = "blockchain_getBlockHash", blocking)]
    fn block_hash(&self, height: u32) -> Result<Option<BlockHash>, Error>;

    // Get block height by hash.
    #[method(name = "blockchain_getBlockNumber", blocking)]
    fn block_number(&self, block_hash: BlockHash) -> Result<Option<u32>, Error>;

    /// Decode a script_pubkey (hex) to a Bitcoin address.
    ///
    /// Returns `None` if the script cannot be converted to an address
    /// (e.g., OP_RETURN outputs, non-standard scripts).
    #[method(name = "blockchain_decodeScriptPubkey", blocking)]
    fn decode_script_pubkey(&self, script_pubkey_hex: String) -> Result<Option<String>, Error>;
}

/// This struct provides the Bitcoin Blockchain API.
pub struct Blockchain<Block, Client, TransactionAdapter> {
    client: Arc<Client>,
    transaction_indexer: Arc<dyn TransactionIndex + Send + Sync>,
    network: Network,
    _phantom: PhantomData<(Block, TransactionAdapter)>,
}

impl<Block, Client, TransactionAdapter> Blockchain<Block, Client, TransactionAdapter>
where
    Block: BlockT + 'static,
    Client: HeaderBackend<Block> + BlockBackend<Block> + AuxStore + 'static,
{
    /// Constructs a new instance of [`Blockchain`].
    pub fn new(
        client: Arc<Client>,
        transaction_indexer: Arc<dyn TransactionIndex + Send + Sync>,
        network: Network,
    ) -> Self {
        Self {
            client,
            transaction_indexer,
            network,
            _phantom: Default::default(),
        }
    }

    fn substrate_block_hash(&self, bitcoin_hash: Option<BlockHash>) -> Result<Block::Hash, Error> {
        match bitcoin_hash {
            Some(h) => self
                .client
                .substrate_block_hash_for(h)
                .ok_or(Error::SubstrateBlockHashNotFound),
            None => Ok(self.client.info().best_hash),
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
    fn header(&self, hash: Option<BlockHash>) -> Result<Option<BitcoinHeader>, Error> {
        let substrate_block_hash = self.substrate_block_hash(hash)?;

        let header = self
            .client
            .header(substrate_block_hash)?
            .ok_or(Error::HeaderNotFound)?;

        let bitcoin_header =
            extract_bitcoin_block_header::<Block>(&header).map_err(Error::Header)?;

        Ok(Some(bitcoin_header))
    }

    fn block(&self, hash: Option<BlockHash>) -> Result<Option<BitcoinBlock>, Error> {
        let substrate_block_hash = self.substrate_block_hash(hash)?;

        let substrate_block = self
            .client
            .block(substrate_block_hash)?
            .ok_or(Error::BlockNotFound)?
            .block;

        let bitcoin_block = convert_to_bitcoin_block::<Block, TransactionAdapter>(substrate_block)
            .map_err(Error::Header)?;

        Ok(Some(bitcoin_block))
    }

    fn block_by_number(&self, number: Option<u32>) -> Result<Option<BitcoinBlock>, Error> {
        let block_hash = match number {
            Some(number) => Some(self.client.block_hash(number).ok_or(Error::BlockNotFound)?),
            None => None,
        };

        self.block(block_hash)
    }

    fn block_with_txids(&self, hash: Option<BlockHash>) -> Result<Option<BlockWithTxids>, Error> {
        match self.block(hash)? {
            Some(block) => {
                let txids = block.txdata.iter().map(|tx| tx.compute_txid()).collect();
                Ok(Some(BlockWithTxids {
                    header: block.header,
                    txids,
                    txdata: block.txdata,
                }))
            }
            None => Ok(None),
        }
    }

    fn block_with_txids_by_number(
        &self,
        number: Option<u32>,
    ) -> Result<Option<BlockWithTxids>, Error> {
        let block_hash = match number {
            Some(number) => Some(self.client.block_hash(number).ok_or(Error::BlockNotFound)?),
            None => None,
        };

        self.block_with_txids(block_hash)
    }

    fn raw_block(&self, hash: Option<BlockHash>) -> Result<Option<String>, Error> {
        match self.block(hash)? {
            Some(block) => {
                let mut data = Vec::new();
                block.consensus_encode(&mut data)?;
                Ok(Some(hex::encode(&data)))
            }
            None => Ok(None),
        }
    }

    fn raw_block_by_number(&self, number: Option<u32>) -> Result<Option<String>, Error> {
        let block_hash = match number {
            Some(number) => Some(self.client.block_hash(number).ok_or(Error::BlockNotFound)?),
            None => None,
        };

        self.raw_block(block_hash)
    }

    fn transaction(&self, txid: Txid) -> Result<Option<Transaction>, Error> {
        let Some(TxPosition {
            block_number,
            index,
        }) = self.transaction_indexer.tx_index(txid)?
        else {
            return Ok(None);
        };
        let substrate_block_hash =
            self.client
                .hash(block_number.into())?
                .ok_or(sp_blockchain::Error::Backend(format!(
                    "Hash not found for #{block_number}"
                )))?;
        let substrate_block = self
            .client
            .block(substrate_block_hash)?
            .ok_or(Error::BlockNotFound)?
            .block;
        let bitcoin_block = convert_to_bitcoin_block::<Block, TransactionAdapter>(substrate_block)
            .map_err(Error::Header)?;
        Ok(bitcoin_block.txdata.into_iter().nth(index as usize))
    }

    fn best_block_hash(&self) -> Result<BlockHash, Error> {
        let best_substrate_hash = self.client.info().best_hash;
        self.client
            .bitcoin_block_hash_for(best_substrate_hash)
            .ok_or(Error::Other("Best block hash not found".to_string()))
    }

    fn block_hash(&self, height: u32) -> Result<Option<BlockHash>, Error> {
        Ok(self.client.block_hash(height))
    }

    fn block_number(&self, block_hash: BlockHash) -> Result<Option<u32>, Error> {
        Ok(self.client.block_number(block_hash))
    }

    fn decode_script_pubkey(&self, script_pubkey_hex: String) -> Result<Option<String>, Error> {
        let script_bytes = hex::decode(&script_pubkey_hex)
            .map_err(|e| Error::Other(format!("Invalid hex: {e}")))?;

        let script = ScriptBuf::from_bytes(script_bytes);

        // Try standard address types first (P2PKH, P2SH, P2WPKH, P2WSH, P2TR)
        if let Ok(address) = Address::from_script(&script, self.network) {
            return Ok(Some(address.to_string()));
        }

        // Handle P2PK - solve() extracts the pubkey, convert to P2PKH address
        if let TxoutType::PubKey(pubkey) = solve(&script) {
            let address = Address::p2pkh(pubkey.pubkey_hash(), self.network);
            return Ok(Some(address.to_string()));
        }

        // Script doesn't represent a standard address (e.g., OP_RETURN, multisig)
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use bitcoin::BlockHash;

    #[test]
    fn test_block_hash_serde() {
        use std::str::FromStr;

        let block_hash =
            BlockHash::from_str("ef537f25c895bfa782526529a9b63d97aa631564d5d789c2b765448c8635fb6c")
                .expect("failed to parse block hash");
        assert_eq!(
            serde_json::to_string(&block_hash).unwrap(),
            "\"ef537f25c895bfa782526529a9b63d97aa631564d5d789c2b765448c8635fb6c\""
        );
    }
}
