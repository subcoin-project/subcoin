use crate::error::Error;
use bitcoin::consensus::encode::{deserialize_hex, serialize_hex};
use bitcoin::{Transaction, Txid};
use jsonrpsee::proc_macros::rpc;
use sc_client_api::{AuxStore, BlockBackend, HeaderBackend};
use serde::{Deserialize, Serialize};
use sp_runtime::traits::Block as BlockT;
use std::marker::PhantomData;
use std::sync::Arc;
use subcoin_network::{NetworkHandle, NetworkStatus, PeerSync, PeerSyncState};

#[derive(Clone, Serialize, Deserialize)]
pub struct NetworkPeers {
    total: usize,
    available: usize,
    discouraged: usize,
    peer_best: Option<u32>,
    sync_peers: Vec<PeerSync>,
}

#[rpc(client, server)]
pub trait SubcoinApi {
    /// Returns a JSON object representing the serialized, hex-encoded transaction.
    ///
    /// # Arguments
    ///
    /// - `raw_tx`: The transaction hex string.
    #[method(name = "subcoin_decodeRawTransaction", blocking)]
    fn decode_raw_transaction(&self, raw_tx: String) -> Result<serde_json::Value, Error>;

    /// Returns the raw transaction data for given txid.
    #[method(name = "subcoin_getRawTransaction")]
    async fn get_raw_transaction(&self, txid: Txid) -> Result<Option<String>, Error>;

    /// Get overall network status.
    #[method(name = "subcoin_networkStatus")]
    async fn network_status(&self) -> Result<Option<NetworkStatus>, Error>;

    /// Get the sync peers.
    #[method(name = "subcoin_networkPeers")]
    async fn network_peers(&self) -> Result<NetworkPeers, Error>;

    /// Submits a raw transaction (serialized, hex-encoded) to local node and network.
    ///
    /// # Arguments
    ///
    /// - `raw_tx`:  The hex string of the raw transaction.
    #[method(name = "subcoin_sendRawTransaction", blocking)]
    fn send_raw_transaction(&self, raw_tx: String) -> Result<(), Error>;
}

/// This struct provides the Subcoin API.
pub struct Subcoin<Block, Client> {
    #[allow(unused)]
    client: Arc<Client>,
    network_handle: NetworkHandle,
    _phantom: PhantomData<Block>,
}

impl<Block, Client> Subcoin<Block, Client>
where
    Block: BlockT + 'static,
    Client: HeaderBackend<Block> + BlockBackend<Block> + AuxStore + 'static,
{
    /// Constructs a new instance of [`Subcoin`].
    pub fn new(client: Arc<Client>, network_handle: NetworkHandle) -> Self {
        Self {
            client,
            network_handle,
            _phantom: Default::default(),
        }
    }
}

#[async_trait::async_trait]
impl<Block, Client> SubcoinApiServer for Subcoin<Block, Client>
where
    Block: BlockT + 'static,
    Client: HeaderBackend<Block> + BlockBackend<Block> + AuxStore + 'static,
{
    fn decode_raw_transaction(&self, raw_tx: String) -> Result<serde_json::Value, Error> {
        let transaction = deserialize_hex::<Transaction>(&raw_tx)?;
        Ok(serde_json::to_value(&transaction)?)
    }

    async fn get_raw_transaction(&self, txid: Txid) -> Result<Option<String>, Error> {
        let maybe_transaction = self.network_handle.get_transaction(txid).await;
        Ok(maybe_transaction.as_ref().map(serialize_hex))
    }

    async fn network_peers(&self) -> Result<NetworkPeers, Error> {
        let sync_peers = self.network_handle.sync_peers().await;
        let total = sync_peers.len();
        let mut available = 0;
        let mut discouraged = 0;
        let mut peer_best = 0;

        for peer in &sync_peers {
            match peer.state {
                PeerSyncState::Available => available += 1,
                PeerSyncState::Discouraged => discouraged += 1,
                _ => {}
            }

            if peer.best_number > peer_best {
                peer_best = peer.best_number;
            }
        }

        Ok(NetworkPeers {
            total,
            available,
            discouraged,
            peer_best: if peer_best > 0 { Some(peer_best) } else { None },
            sync_peers,
        })
    }

    async fn network_status(&self) -> Result<Option<NetworkStatus>, Error> {
        Ok(self.network_handle.status().await)
    }

    fn send_raw_transaction(&self, raw_tx: String) -> Result<(), Error> {
        self.network_handle
            .send_transaction(deserialize_hex::<Transaction>(&raw_tx)?);
        Ok(())
    }
}
