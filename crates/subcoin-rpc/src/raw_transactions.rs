use crate::error::Error;
use bitcoin::consensus::encode::{deserialize_hex, serialize_hex};
use bitcoin::{Transaction, Txid};
use jsonrpsee::proc_macros::rpc;
use sc_client_api::{AuxStore, BlockBackend, HeaderBackend};
use sp_runtime::traits::Block as BlockT;
use std::marker::PhantomData;
use std::sync::Arc;
use subcoin_network::{NetworkApi, SendTransactionResult};

#[rpc(client, server)]
pub trait RawTransactionsApi {
    /// Returns a JSON object representing the serialized, hex-encoded transaction.
    ///
    /// # Arguments
    ///
    /// - `raw_tx`: The transaction hex string.
    #[method(name = "rawtx_decodeRawTransaction", blocking)]
    fn decode_raw_transaction(&self, raw_tx: String) -> Result<serde_json::Value, Error>;

    /// Returns the raw transaction data for given txid.
    #[method(name = "rawtx_getRawTransaction")]
    async fn get_raw_transaction(&self, txid: Txid) -> Result<Option<String>, Error>;

    /// Submits a raw transaction (serialized, hex-encoded) to local node and network.
    ///
    /// # Arguments
    ///
    /// - `raw_tx`:  The hex string of the raw transaction.
    #[method(name = "rawtx_sendRawTransaction")]
    async fn send_raw_transaction(&self, raw_tx: String) -> Result<SendTransactionResult, Error>;
}

/// This struct provides the RawTransactions API.
pub struct RawTransactions<Block, Client> {
    #[allow(unused)]
    client: Arc<Client>,
    network_api: Arc<dyn NetworkApi>,
    _phantom: PhantomData<Block>,
}

impl<Block, Client> RawTransactions<Block, Client>
where
    Block: BlockT + 'static,
    Client: HeaderBackend<Block> + BlockBackend<Block> + AuxStore + 'static,
{
    /// Constructs a new instance of [`RawTransactions`].
    pub fn new(client: Arc<Client>, network_api: Arc<dyn NetworkApi>) -> Self {
        Self {
            client,
            network_api,
            _phantom: Default::default(),
        }
    }
}

#[async_trait::async_trait]
impl<Block, Client> RawTransactionsApiServer for RawTransactions<Block, Client>
where
    Block: BlockT + 'static,
    Client: HeaderBackend<Block> + BlockBackend<Block> + AuxStore + 'static,
{
    fn decode_raw_transaction(&self, raw_tx: String) -> Result<serde_json::Value, Error> {
        let transaction = deserialize_hex::<Transaction>(&raw_tx)?;
        Ok(serde_json::to_value(transaction)?)
    }

    async fn get_raw_transaction(&self, txid: Txid) -> Result<Option<String>, Error> {
        let maybe_transaction = self.network_api.get_transaction(txid).await;
        Ok(maybe_transaction.as_ref().map(serialize_hex))
    }

    async fn send_raw_transaction(&self, raw_tx: String) -> Result<SendTransactionResult, Error> {
        if !self.network_api.enabled() {
            return Err(Error::NetworkUnavailable);
        }

        Ok(self
            .network_api
            .send_transaction(deserialize_hex::<Transaction>(&raw_tx)?)
            .await)
    }
}
