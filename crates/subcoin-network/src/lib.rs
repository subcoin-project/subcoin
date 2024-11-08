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
mod checkpoint;
mod metrics;
mod network_api;
mod network_processor;
mod peer_connection;
mod peer_manager;
mod peer_store;
mod sync;
#[cfg(test)]
mod tests;
mod transaction_manager;

use crate::metrics::BandwidthMetrics;
use crate::network_api::NetworkProcessorMessage;
use crate::network_processor::NetworkProcessor;
use crate::peer_connection::ConnectionInitiator;
use crate::peer_store::{PersistentPeerStore, PersistentPeerStoreHandle};
use bitcoin::p2p::ServiceFlags;
use bitcoin::{BlockHash, Network as BitcoinNetwork};
use chrono::prelude::{DateTime, Local};
use peer_manager::HandshakeState;
use sc_client_api::{AuxStore, HeaderBackend};
use sc_consensus_nakamoto::{BlockImportQueue, ChainParams, HeaderVerifier};
use sc_network_sync::SyncingService;
use sc_service::TaskManager;
use sc_utils::mpsc::{tracing_unbounded, TracingUnboundedSender};
use sp_runtime::traits::Block as BlockT;
use std::net::{AddrParseError, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Arc;
use substrate_prometheus_endpoint::Registry;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

pub use crate::network_api::{NetworkHandle, NetworkStatus, SendTransactionResult, SyncStatus};
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
    #[error("Bad nonce in pong, expected: {expected}, got: {got}")]
    BadPong { expected: u64, got: u64 },
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

/// Controls the block sync from Bitcoin P2P network.
pub enum BlockSyncOption {
    /// Bitcoin block sync is enabled on startup.
    AlwaysOn,
    /// Bitcoin block sync is fully disabled.
    Off,
    /// Bitcoin block sync is paused until Substrate fast sync completes,
    /// after which it resumes automatically.
    PausedUntilFastSync,
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
    pub block_sync: BlockSyncOption,
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

/// Watch the Substrate sync status and enable the subcoin block sync when the Substate
/// state sync is finished.
// TODO: I'm not super happy with pulling in the dep sc-network-sync just for SyncingService.
async fn watch_substrate_fast_sync<Block>(
    subcoin_network_handle: NetworkHandle,
    substate_sync_service: Arc<SyncingService<Block>>,
) where
    Block: BlockT,
{
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(100));

    let mut state_sync_has_started = false;

    loop {
        interval.tick().await;

        let state_sync_is_active = substate_sync_service
            .status()
            .await
            .map(|status| status.state_sync.is_some())
            .unwrap_or(false);

        if state_sync_is_active {
            if !state_sync_has_started {
                state_sync_has_started = true;
            }
        } else if state_sync_has_started {
            tracing::info!("Detected state sync is complete, starting Subcoin block sync");
            subcoin_network_handle.start_block_sync();
            return;
        }
    }
}

async fn listen_for_inbound_connections(
    listener: TcpListener,
    max_inbound_peers: usize,
    connection_initiator: ConnectionInitiator,
    processor_msg_sender: TracingUnboundedSender<NetworkProcessorMessage>,
) {
    let local_addr = listener
        .local_addr()
        .expect("Local listen addr must be available; qed");

    tracing::info!("🔊 Listening on {local_addr:?}",);

    while let Ok((socket, peer_addr)) = listener.accept().await {
        let (sender, receiver) = oneshot::channel();

        if processor_msg_sender
            .unbounded_send(NetworkProcessorMessage::RequestInboundPeersCount(sender))
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
                tracing::debug!(?err, ?peer_addr, "Failed to initiate inbound connection");
            }
        }
    }
}

async fn initialize_outbound_connections(
    network: bitcoin::Network,
    seednodes: Vec<String>,
    seednode_only: bool,
    persistent_peers: Vec<PeerId>,
    connection_initiator: ConnectionInitiator,
) {
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
}

/// Creates Subcoin network.
pub async fn build_network<Block, Client>(
    client: Arc<Client>,
    config: Config,
    import_queue: BlockImportQueue,
    task_manager: &TaskManager,
    registry: Option<Registry>,
    substrate_sync_service: Option<Arc<SyncingService<Block>>>,
) -> Result<NetworkHandle, Error>
where
    Block: BlockT,
    Client: HeaderBackend<Block> + AuxStore + 'static,
{
    let (processor_msg_sender, processor_msg_receiver) =
        tracing_unbounded("mpsc_subcoin_network_processor", 100);

    let is_major_syncing = Arc::new(AtomicBool::new(false));

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

    let spawn_handle = task_manager.spawn_handle();

    let connection_initiator = ConnectionInitiator::new(
        config.network,
        network_event_sender,
        spawn_handle.clone(),
        bandwidth.clone(),
        config.ipv4_only,
    );

    let (sender, receiver) = tracing_unbounded("mpsc_subcoin_peer_store", 10_000);
    let (persistent_peer_store, persistent_peers) =
        PersistentPeerStore::new(&config.base_path, config.max_outbound_peers);
    let persistent_peer_store_handle =
        PersistentPeerStoreHandle::new(config.persistent_peer_latency_threshold, sender);

    spawn_handle.spawn(
        "peer-store",
        Some("subcoin-networking"),
        persistent_peer_store.run(receiver),
    );

    let Config {
        seednode_only,
        seednodes,
        network,
        max_inbound_peers,
        max_outbound_peers,
        sync_strategy,
        block_sync,
        ..
    } = config;

    let network_handle = NetworkHandle {
        processor_msg_sender: processor_msg_sender.clone(),
        is_major_syncing: is_major_syncing.clone(),
    };

    let enable_block_sync = matches!(block_sync, BlockSyncOption::AlwaysOn);

    if !enable_block_sync {
        tracing::info!("Subcoin block sync is disabled on startup");
    }

    if matches!(block_sync, BlockSyncOption::PausedUntilFastSync) {
        if let Some(substrate_sync_service) = substrate_sync_service {
            spawn_handle.spawn(
                "substrate-fast-sync-watcher",
                None,
                watch_substrate_fast_sync(network_handle.clone(), substrate_sync_service),
            );
        } else {
            tracing::warn!("Block sync from Bitcoin P2P network will not be started automatically on Substrate fast sync completion");
        }
    }

    task_manager.spawn_essential_handle().spawn_blocking(
        "net-processor",
        Some("subcoin-networking"),
        {
            let is_major_syncing = is_major_syncing.clone();
            let connection_initiator = connection_initiator.clone();
            let client = client.clone();

            NetworkProcessor::new(
                network_processor::Params {
                    client: client.clone(),
                    header_verifier: HeaderVerifier::new(client, ChainParams::new(network)),
                    network_event_receiver,
                    import_queue,
                    sync_strategy,
                    is_major_syncing,
                    connection_initiator,
                    max_outbound_peers,
                    enable_block_sync,
                    peer_store: Arc::new(persistent_peer_store_handle),
                },
                registry.as_ref(),
            )
            .run(processor_msg_receiver, bandwidth)
        },
    );

    spawn_handle.spawn(
        "inbound-connection",
        Some("subcoin-networking"),
        listen_for_inbound_connections(
            listener,
            max_inbound_peers,
            connection_initiator.clone(),
            processor_msg_sender,
        ),
    );

    spawn_handle.spawn(
        "init-outbound-connection",
        Some("subcoin-networking"),
        initialize_outbound_connections(
            network,
            seednodes,
            seednode_only,
            persistent_peers,
            connection_initiator,
        ),
    );

    Ok(network_handle)
}
