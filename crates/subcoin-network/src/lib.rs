//! # Bitcoin Network
//!
//! This crate facilitates communication with other nodes in the Bitcoin P2P network. It handles
//! connections, message transmission, and the peer-to-peer protocol.
//!
//! ## Initial Block Download
//!
//! This crate offers two strategies for the initial block download:
//!
//! - **Blocks-First**: Downloads the full block data sequentially starting from the last known
//! block until it's fully synced with the network, in batches. This is primarily for the testing
//! purpose.
//!
//! - **Headers-First**: First downloads the block headers and then proceeds to the full block data
//! based on the checkpoints.
//!
//! However, due to the nature of Subcoin, building a Bitcoin SPV node solely by syncing Bitcoin headers
//! from the network is not possible, in that Subcoin requires full block data to derive the corresponding
//! substrate header properly, including generating the `extrinsics_root` and `state_root`.
//!
//! ## Subcoin Bootstrap Node
//!
//! Subcoin node runs the Bitcoin networking and Substrate networking in parallel. Initial block download
//! from Bitcoin p2p network is time-consuming because every historical block must be downloaded and executed.
//! The advantage of Subcoin node is that only the Subcoin bootstrap node needs to perform full block sync.
//! Once Subcoin bootstrap nodes are operational in the network, newly joined Subcoin nodes can quickly sync
//! to Bitcoin chain's tip by leveraging the advanced state sync provided by the Substrate networking stack.

mod address_book;
mod block_downloader;
mod checkpoint;
mod connection;
mod orphan_blocks_pool;
mod peer_manager;
mod sync;
#[cfg(test)]
mod tests;
mod worker;

use crate::connection::ConnectionInitiator;
use crate::worker::NetworkWorker;
use bitcoin::p2p::ServiceFlags;
use bitcoin::{BlockHash, Network as BitcoinNetwork};
use peer_manager::HandshakeState;
use sc_client_api::{AuxStore, HeaderBackend};
use sc_consensus_nakamoto::BlockImportQueue;
use sc_service::SpawnTaskHandle;
use sc_utils::mpsc::{tracing_unbounded, TracingUnboundedReceiver, TracingUnboundedSender};
use serde::{Deserialize, Serialize};
use sp_runtime::traits::Block as BlockT;
use std::marker::PhantomData;
use std::net::{AddrParseError, SocketAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

pub use crate::sync::{PeerLatency, PeerSync, PeerSyncState};

/// Identifies a peer.
pub type PeerId = SocketAddr;

/// Peer's ping latency in milliseconds.
pub type Latency = u128;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Invalid bootnode address: {0}")]
    InvalidBootnode(String),
    #[error("Received 0 bytes, peer performed an orderly shutdown")]
    PeerShutdown,
    #[error("Cannot communicate with the network event stream")]
    NetworkEventStreamError,
    #[error("Peer {0:?} not found")]
    PeerNotFound(PeerId),
    #[error("Connection of peer {0:?} not found")]
    ConnectionNotFound(PeerId),
    #[error("Connecting to the stream timed out")]
    ConnectionTimeout,
    #[error("Unexpected handshake state: {0:?}")]
    UnexpectedHandshakeState(Box<HandshakeState>),
    #[error("Only IPv4 peers are supported")]
    Ipv4Only,
    #[error("Peer is not a full node")]
    NotFullNode,
    #[error("Peer is not a segwit node")]
    NotSegwitNode,
    #[error("Peer's protocol version is too low")]
    ProtocolVersionTooLow,
    #[error("Too many block entries in inv message")]
    TooManyBlockEntries,
    #[error("Too many headers (> 2000)")]
    TooManyHeaders,
    #[error("Too many inventory items")]
    TooManyInventoryItems,
    #[error("Ping timeout")]
    PingTimeout,
    #[error("Ping latency exceeds the threshold")]
    PingLatencyTooHigh,
    #[error("Peer's latency ({0} ms) is too high")]
    SlowPeer(Latency),
    #[error("Unexpected pong message")]
    UnexpectedPong,
    #[error("Invalid pong message: bad nonce")]
    BadPong,
    #[error("Received an unrequested block: {0:?}")]
    UnrequestedBlock(BlockHash),
    #[error("Other: {0}")]
    Other(String),
    #[error(transparent)]
    IO(#[from] std::io::Error),
    #[error(transparent)]
    InvalidAddress(#[from] AddrParseError),
    #[error(transparent)]
    Blockchain(#[from] sp_blockchain::Error),
    #[error(transparent)]
    Consensus(#[from] sp_consensus::Error),
    #[error(transparent)]
    BitcoinIO(#[from] bitcoin::io::Error),
    #[error(transparent)]
    BitcoinEncoding(#[from] bitcoin::consensus::encode::Error),
}

fn seednodes(network: BitcoinNetwork) -> Vec<&'static str> {
    match network {
        BitcoinNetwork::Bitcoin => {
            vec![
                "seed.bitcoin.sipa.be:8333",                        // Pieter Wuille
                "dnsseed.bluematt.me:8333",                         // Matt Corallo
                "dnsseed.bitcoin.dashjr-list-of-p2p-nodes.us:8333", // Luke Dashjr
                "seed.bitcoinstats.com:8333",                       // Christian Decker
                "seed.bitcoin.jonasschnelli.ch:8333",               // Jonas Schnelli
                "seed.btc.petertodd.net:8333",                      // Peter Todd
                "seed.bitcoin.sprovoost.nl:8333",                   // Sjors Provoost
                "dnsseed.emzy.de:8333",                             // Stephan Oeste
                "seed.bitcoin.wiz.biz:8333",                        // Jason Maurice
                "seed.mainnet.achownodes.xyz:8333",                 // Ava Chow
            ]
        }
        BitcoinNetwork::Testnet => {
            vec![
                "testnet-seed.bitcoin.jonasschnelli.ch:18333",
                "seed.tbtc.petertodd.net:18333",
                "seed.testnet.bitcoin.sprovoost.nl:18333",
                "testnet-seed.bluematt.me:18333",
                "testnet-seed.achownodes.xyz:18333",
            ]
        }
        _ => Vec::new(),
    }
}

// Ignore the peer if it is not full with witness enabled as we only want to
// download from peers that can provide use full witness data for blocks.
fn validate_outbound_services(services: ServiceFlags) -> Result<(), Error> {
    if !services.has(ServiceFlags::NETWORK) {
        return Err(Error::NotFullNode);
    }

    if !services.has(ServiceFlags::WITNESS) {
        return Err(Error::NotSegwitNode);
    }

    Ok(())
}

/// Represents the strategy for block syncing.
#[derive(Debug, Clone, Copy, Default)]
#[cfg_attr(feature = "cli", derive(clap::ValueEnum))]
pub enum SyncStrategy {
    /// Download the headers first, followed by the block bodies.
    #[default]
    HeadersFirst,
    /// Download the full blocks (both headers and bodies) in sequence.
    BlocksFirst,
}

/// Represents the sync status of node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SyncStatus {
    /// The node is idle and not currently major syncing.
    Idle,
    /// The node is downloading blocks from peers.
    ///
    /// `target` specifies the block number the node aims to reach.
    /// `peers` is a list of peers from which the node is downloading.
    Downloading { target: u32, peers: Vec<PeerId> },
    /// The node is importing downloaded blocks into the local database.
    Importing { target: u32, peers: Vec<PeerId> },
}

/// Represents the status of network.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkStatus {
    /// The number of peers currently connected to the node.
    pub num_connected_peers: usize,
    /// The total number of bytes received from the network.
    pub total_bytes_inbound: u64,
    /// The total number of bytes sent to the network.
    pub total_bytes_outbound: u64,
    /// Current sync status of the node.
    pub sync_status: SyncStatus,
}

#[derive(Debug, Default)]
struct Bandwidth {
    total_bytes_inbound: Arc<AtomicU64>,
    total_bytes_outbound: Arc<AtomicU64>,
}

impl Clone for Bandwidth {
    fn clone(&self) -> Self {
        Self {
            total_bytes_inbound: self.total_bytes_inbound.clone(),
            total_bytes_outbound: self.total_bytes_outbound.clone(),
        }
    }
}

/// Message to the network worker.
#[derive(Debug)]
enum NetworkWorkerMessage {
    /// Retrieve the network status.
    NetworkStatus(oneshot::Sender<NetworkStatus>),
    /// Retrieve the sync peers.
    SyncPeers(oneshot::Sender<Vec<PeerSync>>),
    /// Retrieve the number of inbound connected peers.
    InboundPeersCount(oneshot::Sender<usize>),
}

/// A handle for interacting with the network worker.
///
/// This handle allows sending messages to the network worker and provides a simple
/// way to check if the node is performing a major synchronization.
#[derive(Debug, Clone)]
pub struct NetworkHandle {
    worker_msg_sender: TracingUnboundedSender<NetworkWorkerMessage>,
    // A simple flag to know whether the node is doing the major sync.
    is_major_syncing: Arc<AtomicBool>,
}

impl NetworkHandle {
    /// Provides high-level status information about network.
    ///
    /// Returns None if the `NetworkWorker` is no longer running.
    pub async fn status(&self) -> Option<NetworkStatus> {
        let (sender, receiver) = oneshot::channel();

        self.worker_msg_sender
            .unbounded_send(NetworkWorkerMessage::NetworkStatus(sender))
            .ok();

        receiver.await.ok()
    }

    /// Returns the currently syncing peers.
    pub async fn sync_peers(&self) -> Vec<PeerSync> {
        let (sender, receiver) = oneshot::channel();

        if self
            .worker_msg_sender
            .unbounded_send(NetworkWorkerMessage::SyncPeers(sender))
            .is_err()
        {
            return Vec::new();
        }

        receiver.await.unwrap_or_default()
    }

    /// Returns a flag indicating whether the node is actively performing a major sync.
    pub fn is_major_syncing(&self) -> Arc<AtomicBool> {
        self.is_major_syncing.clone()
    }
}

/// Network params.
pub struct Params {
    /// Bitcoin network type.
    pub network: BitcoinNetwork,
    /// Specify the local listen address.
    pub listen_on: PeerId,
    /// List of seednodes.
    pub bootnodes: Vec<String>,
    /// Whether to connect to the bootnode only.
    pub bootnode_only: bool,
    /// Whether to accept the peer in ipv4 only.
    pub ipv4_only: bool,
    /// Maximum number of outbound peer connections.
    pub max_outbound_peers: usize,
    /// Maximum number of inbound peer connections.
    pub max_inbound_peers: usize,
    /// Major sync strategy.
    pub sync_strategy: SyncStrategy,
}

impl Params {
    fn bootnodes(&self) -> Vec<String> {
        if self.bootnode_only {
            self.bootnodes.clone()
        } else {
            let mut nodes: Vec<String> = seednodes(self.network)
                .into_iter()
                .map(Into::into)
                .collect();
            nodes.extend_from_slice(&self.bootnodes);
            nodes
        }
    }
}

/// Represents the network component.
pub struct Network<Block, Client> {
    client: Arc<Client>,
    params: Params,
    import_queue: BlockImportQueue,
    spawn_handle: SpawnTaskHandle,
    worker_msg_sender: TracingUnboundedSender<NetworkWorkerMessage>,
    worker_msg_receiver: TracingUnboundedReceiver<NetworkWorkerMessage>,
    is_major_syncing: Arc<AtomicBool>,
    _phantom: PhantomData<Block>,
}

impl<Block, Client> Network<Block, Client>
where
    Block: BlockT,
    Client: HeaderBackend<Block> + AuxStore,
{
    /// Constructs a new instance of [`Network`].
    pub fn new(
        client: Arc<Client>,
        params: Params,
        import_queue: BlockImportQueue,
        spawn_handle: SpawnTaskHandle,
    ) -> (Self, NetworkHandle) {
        let (worker_msg_sender, worker_msg_receiver) = tracing_unbounded("network_worker_msg", 100);

        let is_major_syncing = Arc::new(AtomicBool::new(false));

        let network = Self {
            client,
            params,
            import_queue,
            spawn_handle,
            worker_msg_sender: worker_msg_sender.clone(),
            worker_msg_receiver,
            is_major_syncing: is_major_syncing.clone(),
            _phantom: Default::default(),
        };

        (
            network,
            NetworkHandle {
                worker_msg_sender,
                is_major_syncing,
            },
        )
    }

    /// Starts the network.
    ///
    /// This must be run in a background task.
    pub async fn run(self) -> Result<(), Error> {
        let Self {
            client,
            params,
            import_queue,
            spawn_handle,
            worker_msg_sender,
            worker_msg_receiver,
            is_major_syncing,
            _phantom,
        } = self;

        let mut listen_on = params.listen_on;
        let listener = match TcpListener::bind(&listen_on).await {
            Ok(listener) => listener,
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                tracing::warn!("{listen_on} is occupied, trying any available port.");
                listen_on.set_port(0);
                TcpListener::bind(listen_on).await?
            }
            Err(err) => return Err(err.into()),
        };

        let (network_event_sender, network_event_receiver) = tokio::sync::mpsc::unbounded_channel();

        let bandwidth = Bandwidth::default();

        let connection_initiator = ConnectionInitiator::new(
            params.network,
            network_event_sender,
            spawn_handle.clone(),
            bandwidth.clone(),
            params.ipv4_only,
        );

        let network_worker = NetworkWorker::new(
            client.clone(),
            network_event_receiver,
            import_queue,
            params.sync_strategy,
            is_major_syncing,
            connection_initiator.clone(),
            params.max_outbound_peers,
        );

        spawn_handle.spawn("inbound-connection", None, {
            let local_addr = listener.local_addr()?;
            let connection_initiator = connection_initiator.clone();
            let max_inbound_peers = params.max_inbound_peers;

            async move {
                tracing::info!("🔊 Listening on {local_addr:?}",);

                while let Ok((socket, peer_addr)) = listener.accept().await {
                    let (sender, receiver) = oneshot::channel();

                    if worker_msg_sender
                        .unbounded_send(NetworkWorkerMessage::InboundPeersCount(sender))
                        .is_err()
                    {
                        return;
                    }

                    let Ok(inbound_peers_count) = receiver.await else {
                        return;
                    };

                    if inbound_peers_count < max_inbound_peers {
                        tracing::debug!(?peer_addr, "New peer accepted");

                        if let Err(err) = connection_initiator.initiate_inbound_connection(socket) {
                            tracing::debug!(
                                ?err,
                                ?peer_addr,
                                "Failed to initiate inbound connection"
                            );
                        }
                    }
                }
            }
        });

        for bootnode in params.bootnodes() {
            connection_initiator.initiate_outbound_connection(
                bootnode
                    .to_socket_addrs()?
                    .next()
                    .ok_or_else(|| Error::InvalidBootnode(bootnode.to_string()))?,
            );
        }

        network_worker.run(worker_msg_receiver, bandwidth).await;

        Ok(())
    }
}
