use crate::error::Error;
use jsonrpsee::proc_macros::rpc;
use sc_client_api::{AuxStore, BlockBackend, HeaderBackend};
use serde::{Deserialize, Serialize};
use sp_runtime::traits::Block as BlockT;
use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::sync::Arc;
use subcoin_network::{NetworkHandle, NetworkStatus, PeerSync, PeerSyncState};

/// The state of syncing between a Peer and ourselves.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize, Hash)]
#[serde(rename_all = "camelCase")]
pub enum SyncState {
    Available,
    Deprioritized,
    DownloadingNew,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkPeers {
    /// A map containing the count of peers in each sync state (e.g., syncing, idle).
    sync_state_counts: BTreeMap<SyncState, usize>,
    /// The highest block height known across all peers in the network.
    best_known_block: Option<u32>,
    /// Detailed synchronization information for each peer.
    peer_sync_details: Vec<PeerSync>,
}

#[rpc(client, server)]
pub trait NetworkApi {
    /// Get the sync peers.
    #[method(name = "network_peers")]
    async fn network_peers(&self) -> Result<NetworkPeers, Error>;

    /// Get overall network status.
    #[method(name = "network_status")]
    async fn network_status(&self) -> Result<Option<NetworkStatus>, Error>;
}

/// This struct provides the Network API.
pub struct Network<Block, Client> {
    #[allow(unused)]
    client: Arc<Client>,
    network_handle: NetworkHandle,
    _phantom: PhantomData<Block>,
}

impl<Block, Client> Network<Block, Client>
where
    Block: BlockT + 'static,
    Client: HeaderBackend<Block> + BlockBackend<Block> + AuxStore + 'static,
{
    /// Constructs a new instance of [`Network`].
    pub fn new(client: Arc<Client>, network_handle: NetworkHandle) -> Self {
        Self {
            client,
            network_handle,
            _phantom: Default::default(),
        }
    }
}

#[async_trait::async_trait]
impl<Block, Client> NetworkApiServer for Network<Block, Client>
where
    Block: BlockT + 'static,
    Client: HeaderBackend<Block> + BlockBackend<Block> + AuxStore + 'static,
{
    async fn network_peers(&self) -> Result<NetworkPeers, Error> {
        let mut sync_peers = self.network_handle.sync_peers().await;

        let mut available = 0;
        let mut deprioritized = 0;
        let mut downloading_new = 0;

        let mut best_known_block = 0;

        for peer in &sync_peers {
            match peer.state {
                PeerSyncState::Available => available += 1,
                PeerSyncState::Deprioritized { .. } => deprioritized += 1,
                PeerSyncState::DownloadingNew { .. } => downloading_new += 1,
            }

            if peer.best_number > best_known_block {
                best_known_block = peer.best_number;
            }
        }

        sync_peers.sort_by_key(|x| x.latency);

        Ok(NetworkPeers {
            sync_state_counts: BTreeMap::from([
                (SyncState::Available, available),
                (SyncState::Deprioritized, deprioritized),
                (SyncState::DownloadingNew, downloading_new),
            ]),
            best_known_block: if best_known_block > 0 {
                Some(best_known_block)
            } else {
                None
            },
            peer_sync_details: sync_peers,
        })
    }

    async fn network_status(&self) -> Result<Option<NetworkStatus>, Error> {
        Ok(self.network_handle.status().await)
    }
}
