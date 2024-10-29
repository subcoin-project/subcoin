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
//!     block until it's fully synced with the network, in batches. This is primarily for the testing
//!     purpose.
//!
//! - **Headers-First**: First downloads the block headers and then proceeds to the full block data
//!     based on the checkpoints.
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
mod metrics;
mod network;
mod orphan_blocks_pool;
mod peer_manager;
mod peer_store;
mod sync;
#[cfg(test)]
mod tests;
mod transaction_manager;
mod worker;

use crate::connection::ConnectionInitiator;
use crate::metrics::BandwidthMetrics;
use crate::network::NetworkWorkerMessage;
use crate::peer_store::{PersistentPeerStore, PersistentPeerStoreHandle};
use crate::worker::NetworkWorker;
use bitcoin::p2p::ServiceFlags;
use bitcoin::{BlockHash, Network as BitcoinNetwork};
use chrono::prelude::{DateTime, Local};
use peer_manager::HandshakeState;
use sc_client_api::{AuxStore, HeaderBackend};
use sc_consensus_nakamoto::{BlockImportQueue, ChainParams, HeaderVerifier};
use sc_service::SpawnTaskHandle;
use sc_utils::mpsc::{tracing_unbounded, TracingUnboundedReceiver, TracingUnboundedSender};
use sp_runtime::traits::Block as BlockT;
use std::marker::PhantomData;
use std::net::{AddrParseError, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Arc;
use substrate_prometheus_endpoint::Registry;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

pub use crate::network::{NetworkHandle, NetworkStatus, SendTransactionResult, SyncStatus};
pub use crate::sync::{PeerSync, PeerSyncState};

/// Identifies a peer.
pub type PeerId = SocketAddr;

/// Peer latency in milliseconds.
pub type Latency = u128;

pub(crate) type LocalTime = DateTime<Local>;

/// Network error type.
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
    #[error("Handshake timeout")]
    HandshakeTimeout,
    #[error("Only IPv4 peers are supported")]
    Ipv4Only,
    #[error("Peer is not a full node")]
    NotFullNode,
    #[error("Peer is not a segwit node")]
    NotSegwitNode,
    #[error("Peer's protocol version is too low")]
    ProtocolVersionTooLow,
    #[error("Header contains invalid proof-of-block")]
    BadProofOfWork(BlockHash),
    #[error("Too many Inventory::Block items in inv message")]
    InvHasTooManyBlockItems,
    #[error("Too many entries (> 2000) in headers message")]
    TooManyHeaders,
    #[error("Entries in headers message are not in ascending order")]
    HeadersNotInAscendingOrder,
    #[error("Too many inventory items")]
    TooManyInventoryItems,
    #[error("Ping timeout")]
    PingTimeout,
    #[error("Ping latency ({0}) exceeds the threshold")]
    PingLatencyTooHigh(Latency),
    #[error("Peer is deprioritized for syncing and has encountered multiple failures")]
    UnreliablePeer,
    #[error("Peer's latency ({0} ms) is too high")]
    SlowPeer(Latency),
    #[error("Unexpected pong message")]
    UnexpectedPong,
    #[error("Invalid pong message: bad nonce")]
    BadPong,
    #[error("Cannot find the parent of the first header in headers message")]
    MissingFirstHeaderParent,
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

#[derive(Debug, Default)]
struct Bandwidth {
    total_bytes_inbound: Arc<AtomicU64>,
    total_bytes_outbound: Arc<AtomicU64>,
    metrics: Option<BandwidthMetrics>,
}

impl Bandwidth {
    fn new(registry: Option<&Registry>) -> Self {
        Self {
            total_bytes_inbound: Arc::new(0.into()),
            total_bytes_outbound: Arc::new(0.into()),
            metrics: registry.and_then(|registry| {
                BandwidthMetrics::register(registry)
                    .map_err(|err| tracing::error!("Failed to register bandwidth metrics: {err}"))
                    .ok()
            }),
        }
    }

    /// Report the metrics if needed.
    ///
    /// Possible labels: `in` and `out`.
    fn report(&self, label: &str, value: u64) {
        if let Some(metrics) = &self.metrics {
            metrics.bandwidth.with_label_values(&[label]).set(value);
        }
    }
}

impl Clone for Bandwidth {
    fn clone(&self) -> Self {
        Self {
            total_bytes_inbound: self.total_bytes_inbound.clone(),
            total_bytes_outbound: self.total_bytes_outbound.clone(),
            metrics: self.metrics.clone(),
        }
    }
}

/// Network configuration.
pub struct Config {
    /// Bitcoin network type.
    pub network: BitcoinNetwork,
    /// Node base path.
    pub base_path: PathBuf,
    /// Specify the local listen address.
    pub listen_on: PeerId,
    /// List of seednodes.
    pub seednodes: Vec<String>,
    /// Whether to connect to the seednode only.
    pub seednode_only: bool,
    /// Whether to accept the peer in ipv4 only.
    pub ipv4_only: bool,
    /// Maximum number of outbound peer connections.
    pub max_outbound_peers: usize,
    /// Maximum number of inbound peer connections.
    pub max_inbound_peers: usize,
    /// Persistent peer latency threshold in milliseconds (ms).
    pub persistent_peer_latency_threshold: u128,
    /// Major sync strategy.
    pub sync_strategy: SyncStrategy,
    /// Whether to enable the block sync on startup.
    ///
    /// The block sync from Bitcoin P2P network may be disabled temporarily when
    /// performing fast sync from the Subcoin network.
    pub enable_block_sync_on_startup: bool,
}

fn builtin_seednodes(network: BitcoinNetwork) -> &'static [&'static str] {
    match network {
        BitcoinNetwork::Bitcoin => {
            &[
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
        BitcoinNetwork::Testnet => &[
            "testnet-seed.bitcoin.jonasschnelli.ch:18333",
            "seed.tbtc.petertodd.net:18333",
            "seed.testnet.bitcoin.sprovoost.nl:18333",
            "testnet-seed.bluematt.me:18333",
            "testnet-seed.achownodes.xyz:18333",
        ],
        _ => &[],
    }
}

/// Represents the Subcoin network component.
pub struct Network<Block, Client> {
    client: Arc<Client>,
    config: Config,
    import_queue: BlockImportQueue,
    spawn_handle: SpawnTaskHandle,
    worker_msg_sender: TracingUnboundedSender<NetworkWorkerMessage>,
    worker_msg_receiver: TracingUnboundedReceiver<NetworkWorkerMessage>,
    is_major_syncing: Arc<AtomicBool>,
    registry: Option<Registry>,
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
        config: Config,
        import_queue: BlockImportQueue,
        spawn_handle: SpawnTaskHandle,
        registry: Option<Registry>,
    ) -> (Self, NetworkHandle) {
        let (worker_msg_sender, worker_msg_receiver) =
            tracing_unbounded("mpsc_subcoin_network_worker", 100);

        let is_major_syncing = Arc::new(AtomicBool::new(false));

        let network = Self {
            client,
            config,
            import_queue,
            spawn_handle,
            worker_msg_sender: worker_msg_sender.clone(),
            worker_msg_receiver,
            is_major_syncing: is_major_syncing.clone(),
            registry,
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
            config,
            import_queue,
            spawn_handle,
            worker_msg_sender,
            worker_msg_receiver,
            is_major_syncing,
            registry,
            _phantom,
        } = self;

        let mut listen_on = config.listen_on;
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

        let bandwidth = Bandwidth::new(registry.as_ref());

        let connection_initiator = ConnectionInitiator::new(
            config.network,
            network_event_sender,
            spawn_handle.clone(),
            bandwidth.clone(),
            config.ipv4_only,
        );

        if !config.enable_block_sync_on_startup {
            tracing::info!("Subcoin block sync is disabled until Substrate fast sync is complete");
        }

        let (sender, receiver) = tracing_unbounded("mpsc_subcoin_peer_store", 10_000);
        let (persistent_peer_store, persistent_peers) =
            PersistentPeerStore::new(&config.base_path, config.max_outbound_peers);
        let persistent_peer_store_handle =
            PersistentPeerStoreHandle::new(config.persistent_peer_latency_threshold, sender);

        spawn_handle.spawn("peer-store", None, persistent_peer_store.run(receiver));

        let network_worker = NetworkWorker::new(
            worker::Params {
                client: client.clone(),
                header_verifier: HeaderVerifier::new(
                    client.clone(),
                    ChainParams::new(config.network),
                ),
                network_event_receiver,
                import_queue,
                sync_strategy: config.sync_strategy,
                is_major_syncing,
                connection_initiator: connection_initiator.clone(),
                max_outbound_peers: config.max_outbound_peers,
                enable_block_sync: config.enable_block_sync_on_startup,
                peer_store: Arc::new(persistent_peer_store_handle),
            },
            registry.as_ref(),
        );

        spawn_handle.spawn("inbound-connection", None, {
            let local_addr = listener.local_addr()?;
            let connection_initiator = connection_initiator.clone();
            let max_inbound_peers = config.max_inbound_peers;

            async move {
                tracing::info!("🔊 Listening on {local_addr:?}",);

                while let Ok((socket, peer_addr)) = listener.accept().await {
                    let (sender, receiver) = oneshot::channel();

                    if worker_msg_sender
                        .unbounded_send(NetworkWorkerMessage::RequestInboundPeersCount(sender))
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

        let Config {
            seednode_only,
            seednodes,
            network,
            ..
        } = config;

        let mut bootnodes = seednodes;

        if !seednode_only {
            bootnodes.extend(builtin_seednodes(network).iter().map(|s| s.to_string()));
        }

        bootnodes.extend(persistent_peers.into_iter().map(|s| s.to_string()));

        // Create a vector of futures for DNS lookups
        let lookup_futures = bootnodes.into_iter().map(|bootnode| async move {
            tokio::net::lookup_host(&bootnode).await.map(|mut addrs| {
                addrs
                    .next()
                    .ok_or_else(|| Error::InvalidBootnode(bootnode.to_string()))
            })
        });

        // Await all futures concurrently
        let lookup_results = futures::future::join_all(lookup_futures).await;

        for result in lookup_results {
            match result {
                Ok(Ok(addr)) => {
                    connection_initiator.initiate_outbound_connection(addr);
                }
                Ok(Err(e)) => {
                    tracing::error!("Failed to resolve bootnode address: {e}");
                }
                Err(e) => {
                    tracing::error!("Failed to perform bootnode DNS lookup: {e}");
                }
            }
        }

        network_worker.run(worker_msg_receiver, bandwidth).await;

        Ok(())
    }
}
