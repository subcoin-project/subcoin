use crate::error::Error;
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
    /// Get overall network status.
    #[method(name = "subcoin_networkStatus")]
    async fn network_status(&self) -> Result<Option<NetworkStatus>, Error>;

    /// Get the sync peers.
    #[method(name = "subcoin_networkPeers")]
    async fn network_peers(&self) -> Result<NetworkPeers, Error>;
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
    /// Constructs a new instance of [`Blockchain`].
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
    async fn network_status(&self) -> Result<Option<NetworkStatus>, Error> {
        Ok(self.network_handle.status().await)
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
}
