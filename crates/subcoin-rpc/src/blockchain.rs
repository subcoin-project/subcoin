use crate::error::Error;
use bitcoin::blockdata::block::Header as BitcoinHeader;
use bitcoin::consensus::Encodable;
use bitcoin::{Block as BitcoinBlock, BlockHash, Transaction, Txid};
use jsonrpsee::proc_macros::rpc;
use sc_client_api::{AuxStore, BlockBackend, HeaderBackend};
use sp_runtime::traits::Block as BlockT;
use std::marker::PhantomData;
use std::sync::Arc;
use subcoin_primitives::{
    convert_to_bitcoin_block, extract_bitcoin_block_header, BackendExt, BitcoinTransactionAdapter,
    TransactionIndex, TxPosition,
};

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

    /// Get a raw block in hex format by its hash.
    #[method(name = "blockchain_getRawBlock", blocking)]
    fn raw_block(&self, hash: Option<BlockHash>) -> Result<Option<String>, Error>;

    /// Get a raw block in hex format by its number.
    #[method(name = "blockchain_getRawBlockByNumber", blocking)]
    fn raw_block_by_number(&self, number: Option<u32>) -> Result<Option<String>, Error>;

    /// Get transaction.
    #[method(name = "blockchain_getTransaction", blocking)]
    fn transaction(&self, txid: Txid) -> Result<Option<Transaction>, Error>;

    // Get current block height
    #[method(name = "blockchain_getBestBlockHash", blocking)]
    fn best_block_hash(&self) -> Result<BlockHash, Error>;

    // Get block hash by height
    #[method(name = "blockchain_getBlockHash", blocking)]
    fn block_hash(&self, height: u32) -> Result<Option<BlockHash>, Error>;

    // Get block height by hash.
    #[method(name = "blockchain_getBlockNumber", blocking)]
    fn block_number(&self, block_hash: BlockHash) -> Result<Option<u32>, Error>;
}

/// This struct provides the Bitcoin Blockchain API.
pub struct Blockchain<Block, Client, TransactionAdapter> {
    client: Arc<Client>,
    transaction_indexer: Arc<dyn TransactionIndex + Send + Sync>,
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
    ) -> Self {
        Self {
            client,
            transaction_indexer,
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
