//! Address-related RPC methods.

use crate::error::Error;
use bitcoin::Txid;
use jsonrpsee::proc_macros::rpc;
use sc_client_api::HeaderBackend;
use serde::{Deserialize, Serialize};
use sp_runtime::traits::{Block as BlockT, SaturatedConversion};
use std::sync::Arc;
use subcoin_indexer::IndexerQuery;

/// Address balance information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddressBalance {
    /// Confirmed balance in satoshis.
    pub confirmed: u64,
    /// Total received in satoshis.
    pub total_received: u64,
    /// Total sent in satoshis.
    pub total_sent: u64,
    /// Number of transactions.
    pub tx_count: u64,
    /// Number of unspent outputs.
    pub utxo_count: u64,
}

/// Transaction in address history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddressTransaction {
    /// Transaction ID.
    pub txid: Txid,
    /// Block height.
    pub block_height: u32,
    /// Net change in satoshis (positive = received, negative = sent).
    pub delta: i64,
    /// Block timestamp.
    pub timestamp: u32,
}

/// Unspent transaction output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddressUtxo {
    /// Transaction ID.
    pub txid: Txid,
    /// Output index.
    pub vout: u32,
    /// Value in satoshis.
    pub value: u64,
    /// Block height.
    pub block_height: u32,
    /// ScriptPubKey as hex.
    pub script_pubkey: String,
}

/// Indexer status information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexerStatus {
    /// Whether the indexer is currently syncing.
    pub is_syncing: bool,
    /// Current indexed block height.
    pub indexed_height: u32,
    /// Target block height (during sync).
    pub target_height: Option<u32>,
    /// Sync progress as percentage (0.0 - 100.0).
    pub progress_percent: f64,
}

/// Address statistics and summary information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddressStats {
    /// Block height when address first received funds.
    pub first_seen_height: Option<u32>,
    /// Timestamp when address first received funds.
    pub first_seen_timestamp: Option<u32>,
    /// Block height of most recent transaction.
    pub last_seen_height: Option<u32>,
    /// Timestamp of most recent transaction.
    pub last_seen_timestamp: Option<u32>,
    /// Largest single receive amount in satoshis.
    pub largest_receive: u64,
    /// Largest single send amount in satoshis (absolute value).
    pub largest_send: u64,
    /// Total number of receive transactions.
    pub receive_count: u64,
    /// Total number of send transactions.
    pub send_count: u64,
}

/// Address API.
#[rpc(client, server)]
pub trait AddressApi {
    /// Get address balance.
    #[method(name = "address_getBalance")]
    async fn get_balance(&self, address: String) -> Result<AddressBalance, Error>;

    /// Get address transaction history with pagination.
    #[method(name = "address_getHistory")]
    async fn get_history(
        &self,
        address: String,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<AddressTransaction>, Error>;

    /// Get address UTXOs.
    #[method(name = "address_getUtxos")]
    async fn get_utxos(&self, address: String) -> Result<Vec<AddressUtxo>, Error>;

    /// Get address transaction count.
    #[method(name = "address_getTxCount")]
    async fn get_tx_count(&self, address: String) -> Result<u64, Error>;

    /// Get indexer status (sync progress).
    #[method(name = "address_indexerStatus")]
    async fn indexer_status(&self) -> Result<IndexerStatus, Error>;

    /// Get address statistics (first/last seen, largest tx, etc).
    #[method(name = "address_getStats")]
    async fn get_stats(&self, address: String) -> Result<AddressStats, Error>;
}

/// Address RPC implementation.
pub struct Address<Block, Client> {
    client: Arc<Client>,
    indexer_query: Arc<IndexerQuery>,
    _phantom: std::marker::PhantomData<Block>,
}

impl<Block, Client> Address<Block, Client>
where
    Block: BlockT,
    Client: HeaderBackend<Block>,
{
    /// Create a new Address RPC.
    pub fn new(client: Arc<Client>, indexer_query: Arc<IndexerQuery>) -> Self {
        Self {
            client,
            indexer_query,
            _phantom: std::marker::PhantomData,
        }
    }
}

#[async_trait::async_trait]
impl<Block, Client> AddressApiServer for Address<Block, Client>
where
    Block: BlockT + 'static,
    Client: HeaderBackend<Block> + Send + Sync + 'static,
{
    async fn get_balance(&self, address: String) -> Result<AddressBalance, Error> {
        let balance = self
            .indexer_query
            .get_address_balance(&address)
            .await
            .map_err(|e| Error::Other(e.to_string()))?;

        Ok(AddressBalance {
            confirmed: balance.confirmed,
            total_received: balance.total_received,
            total_sent: balance.total_sent,
            tx_count: balance.tx_count,
            utxo_count: balance.utxo_count,
        })
    }

    async fn get_history(
        &self,
        address: String,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<AddressTransaction>, Error> {
        let history = self
            .indexer_query
            .get_address_history(&address, limit.unwrap_or(25), offset.unwrap_or(0))
            .await
            .map_err(|e| Error::Other(e.to_string()))?;

        Ok(history
            .into_iter()
            .map(|h| AddressTransaction {
                txid: h.txid,
                block_height: h.block_height,
                delta: h.delta,
                timestamp: h.timestamp,
            })
            .collect())
    }

    async fn get_utxos(&self, address: String) -> Result<Vec<AddressUtxo>, Error> {
        let utxos = self
            .indexer_query
            .get_address_utxos(&address)
            .await
            .map_err(|e| Error::Other(e.to_string()))?;

        Ok(utxos
            .into_iter()
            .map(|u| AddressUtxo {
                txid: u.txid,
                vout: u.vout,
                value: u.value,
                block_height: u.block_height,
                script_pubkey: hex::encode(&u.script_pubkey),
            })
            .collect())
    }

    async fn get_tx_count(&self, address: String) -> Result<u64, Error> {
        self.indexer_query
            .get_address_tx_count(&address)
            .await
            .map_err(|e| Error::Other(e.to_string()))
    }

    async fn indexer_status(&self) -> Result<IndexerStatus, Error> {
        let chain_tip: u32 = self.client.info().best_number.saturated_into();
        let status = self
            .indexer_query
            .get_status(chain_tip)
            .await
            .map_err(|e| Error::Other(e.to_string()))?;

        Ok(IndexerStatus {
            is_syncing: status.is_syncing,
            indexed_height: status.indexed_height,
            target_height: status.target_height,
            progress_percent: status.progress_percent,
        })
    }

    async fn get_stats(&self, address: String) -> Result<AddressStats, Error> {
        let stats = self
            .indexer_query
            .get_address_stats(&address)
            .await
            .map_err(|e| Error::Other(e.to_string()))?;

        Ok(AddressStats {
            first_seen_height: stats.first_seen_height,
            first_seen_timestamp: stats.first_seen_timestamp,
            last_seen_height: stats.last_seen_height,
            last_seen_timestamp: stats.last_seen_timestamp,
            largest_receive: stats.largest_receive,
            largest_send: stats.largest_send,
            receive_count: stats.receive_count,
            send_count: stats.send_count,
        })
    }
}
