use crate::error::Error;
use bitcoin::blockdata::block::Header as BitcoinHeader;
use bitcoin::{Block as BitcoinBlock, BlockHash};
use jsonrpsee::proc_macros::rpc;
use sc_client_api::{AuxStore, BlockBackend, HeaderBackend};
use sp_runtime::traits::Block as BlockT;
use std::marker::PhantomData;
use std::sync::Arc;
use subcoin_primitives::{
    convert_to_bitcoin_block, extract_bitcoin_block_header, BackendExt, BitcoinTransactionAdapter,
};

/// Bitcoin blockchain API.
#[rpc(client, server)]
pub trait BlockchainApi {
    /// Get header.
    #[method(name = "btc_getHeader", blocking)]
    fn header(&self, hash: Option<BlockHash>) -> Result<Option<BitcoinHeader>, Error>;

    /// Get header and body of a block.
    #[method(name = "btc_getBlock", blocking)]
    fn block(&self, hash: Option<BlockHash>) -> Result<Option<BitcoinBlock>, Error>;

    /*
    /// Get hash of the n-th block in the canon chain.
    ///
    /// By default returns latest block hash.
    #[method(name = "chain_getBlockHash", aliases = ["chain_getHead"], blocking)]
    fn block_hash(
        &self,
        hash: Option<ListOrValue<NumberOrHex>>,
    ) -> Result<ListOrValue<Option<Hash>>, Error>;

    /// Get hash of the last finalized block in the canon chain.
    #[method(name = "chain_getFinalizedHead", aliases = ["chain_getFinalisedHead"], blocking)]
    fn finalized_head(&self) -> Result<Hash, Error>;

    /// All head subscription.
    #[subscription(
        name = "chain_subscribeAllHeads" => "chain_allHead",
        unsubscribe = "chain_unsubscribeAllHeads",
        item = Header
    )]
    fn subscribe_all_heads(&self);

    /// New head subscription.
    #[subscription(
        name = "chain_subscribeNewHeads" => "chain_newHead",
        aliases = ["subscribe_newHead", "chain_subscribeNewHead"],
        unsubscribe = "chain_unsubscribeNewHeads",
        unsubscribe_aliases = ["unsubscribe_newHead", "chain_unsubscribeNewHead"],
        item = Header
    )]
    fn subscribe_new_heads(&self);

    /// Finalized head subscription.
    #[subscription(
        name = "chain_subscribeFinalizedHeads" => "chain_finalizedHead",
        aliases = ["chain_subscribeFinalisedHeads"],
        unsubscribe = "chain_unsubscribeFinalizedHeads",
        unsubscribe_aliases = ["chain_unsubscribeFinalisedHeads"],
        item = Header
    )]
    fn subscribe_finalized_heads(&self);
    */
}

/// This struct provides the Bitcoin Blockchain API.
pub struct Blockchain<Block, Client, TransactionAdapter> {
    client: Arc<Client>,
    _phantom: PhantomData<(Block, TransactionAdapter)>,
}

impl<Block, Client, TransactionAdapter> Blockchain<Block, Client, TransactionAdapter>
where
    Block: BlockT + 'static,
    Client: HeaderBackend<Block> + BlockBackend<Block> + AuxStore + 'static,
{
    /// Constructs a new instance of [`Blockchain`].
    pub fn new(client: Arc<Client>) -> Self {
        Self {
            client,
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

        let signed_block = self
            .client
            .block(substrate_block_hash)?
            .ok_or(Error::BlockNotFound)?;

        let block = signed_block.block;

        let bitcoin_header =
            extract_bitcoin_block_header::<Block>(block.header()).map_err(Error::Header)?;

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
        println!("==== {:?}", serde_json::to_string(&block_hash).unwrap());
    }
}
