use crate::address_book::AddressBook;
use crate::metrics::Metrics;
use crate::peer_connection::{
    ConnectionCloser, ConnectionInitiator, ConnectionWriter, Direction, NewConnection,
};
use crate::{validate_outbound_services, Error, Latency, LocalTime, PeerId};
use bitcoin::p2p::address::AddrV2Message;
use bitcoin::p2p::message::NetworkMessage;
use bitcoin::p2p::message_network::VersionMessage;
use bitcoin::p2p::{Address, ServiceFlags};
use chrono::prelude::Local;
use sc_client_api::HeaderBackend;
use sp_runtime::traits::Block as BlockT;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::{Duration, Instant};
use subcoin_primitives::ClientExt;

/// "wtxidrelay" command for wtxid-based relay starts with this version.
const WTXID_RELAY_VERSION: u32 = 70016;

/// Maximum number of available addresses in the address book.
const MAX_AVAILABLE_ADDRESSES: usize = 2000;

/// Peer-to-peer protocol version.
pub const PROTOCOL_VERSION: u32 = 70016;

/// Minimum supported peer protocol version.
///
/// This version includes support for the `sendheaders` feature.
pub const MIN_PROTOCOL_VERSION: u32 = 70012;

/// The maximum allowable peer latency in milliseconds before disconnection.
///
/// If a peer's latency exceeds this threshold (2000ms), it will be disconnected immediately.
pub const PEER_LATENCY_THRESHOLD: Latency = 2000;

/// The threshold for classifying a peer as "slow", based on average ping latency in milliseconds.
///
/// A peer is considered slow if its average latency exceeds 500ms. Slow peers may be evicted from
/// the network to maintain overall performance.
const SLOW_PEER_LATENCY: Latency = 500;

/// Interval for evicting the slowest peer, 10 minutes.
///
/// Periodically evict the slowest peer whose latency is above the threshold [`SLOW_PEER_LATENCY`]
/// when the peer set is full. This creates opportunities to connect with potentially better peers.
const EVICTION_INTERVAL: Duration = Duration::from_secs(600);

/// Timeout for the outbound peer to send their version, in seconds.
const HANDSHAKE_TIMEOUT: i64 = 1;

/// Peer configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// Protocol version.
    pub protocol_version: u32,
    /// Services offered by this implementation.
    pub services: ServiceFlags,
    /// Peer addresses to persist connections with.
    pub persistent: Vec<PeerId>,
    /// Our user agent.
    pub user_agent: String,
}

impl Config {
    pub fn new() -> Self {
        let user_agent = format!("/Subcoin:{}/", env!("CARGO_PKG_VERSION"));
        Self {
            protocol_version: PROTOCOL_VERSION,
            services: ServiceFlags::NONE,
            persistent: Vec::new(),
            user_agent,
        }
    }
}

/// Channel for communication with the remote peer.
struct Connection {
    local_addr: PeerId,
    writer: ConnectionWriter,
    closer: ConnectionCloser,
}

impl Connection {
    #[inline]
    fn send(&self, network_message: NetworkMessage) -> std::io::Result<()> {
        self.writer.send(network_message).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::Other,
                "Failed to send network message: connection writer receiver dropped",
            )
        })
    }
}

/// Represents a new peer that was added.
#[derive(Debug, Clone)]
pub struct NewPeer {
    pub peer_id: PeerId,
    pub best_number: u32,
    pub latency: Latency,
}

/// Handshake state.
///
/// # Peer negotiation (handshake)
///
/// ## Outbound handshake
///
/// 1. Send our `version` message.
/// 2. Expect `version` message from remote.
/// 3. Expect `verack` message from remote.
/// 4. Send our `verack` message.
///
/// ## Inbound handshake
///
/// 1. Expect `version` message from remote.
/// 2. Send our `version` message.
/// 3. Send our `verack` message.
/// 4. Expect `verack` message from remote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum HandshakeState {
    /// TCP connection was just opened.
    #[default]
    ConnectionOpened,
    /// Our version message has been sent, awaiting the peer's version message.
    VersionSent { at: LocalTime },
    /// Received "version", awaiting for the "verack" message.
    VersionReceived(VersionMessage),
    /// Received the "verack" message, handshake is complete.
    VerackReceived {
        /// Peer information, if a `version` message was received.
        peer: PeerId,
    },
}

impl HandshakeState {
    /// Checks if the handshake is complete.
    pub fn is_complete(&self) -> bool {
        matches!(self, Self::VerackReceived { .. })
    }
}

#[derive(Debug, Clone, Default)]
pub struct LatencyTracker {
    /// Number of ping responses received.
    received_pongs: u64,
    /// Accumulated total latency in milliseconds.
    total_latency: Latency,
}

impl LatencyTracker {
    /// Updates the ping latency data and returns the latest average latency.
    fn record_pong(&mut self, latency: Latency) -> Latency {
        self.received_pongs = self.received_pongs.saturating_add(1);
        self.total_latency = self.total_latency.saturating_add(latency);
        self.total_latency / self.received_pongs as u128
    }

    fn calc_average(&self) -> Option<Latency> {
        if self.received_pongs == 0 {
            return None;
        }
        Some(self.total_latency / self.received_pongs as u128)
    }
}

#[derive(Debug, Clone)]
pub enum PingState {
    Idle {
        /// Time at which the last pong was received.
        last_pong_at: Instant,
    },
    AwaitingPong {
        /// Time at which the last ping was sent.
        last_ping_at: Instant,
        nonce: u64,
    },
}

impl PingState {
    const PING_TIMEOUT: Duration = Duration::from_secs(30);
    const PING_INTERVAL: Duration = Duration::from_secs(120);

    fn should_ping(&self) -> bool {
        match self {
            Self::Idle { last_pong_at } => last_pong_at.elapsed() >= Self::PING_INTERVAL,
            Self::AwaitingPong { .. } => false,
        }
    }

    fn has_timeout(&self) -> bool {
        match self {
            Self::Idle { .. } => false,
            Self::AwaitingPong { last_ping_at, .. } => last_ping_at.elapsed() >= Self::PING_TIMEOUT,
        }
    }

    fn idle() -> Self {
        Self::Idle {
            last_pong_at: Instant::now(),
        }
    }
}

/// A peer with protocol information.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct PeerInfo {
    /// The peer's best height.
    pub best_height: u32,
    /// The peer's services.
    pub services: ServiceFlags,
    /// Peer's user agent.
    pub user_agent: String,
    /// Address of our node, as seen by the remote.
    pub receiver: Address,
    /// Whether this peer relays transactions.
    pub relay: bool,
    /// Whether this peer supports BIP-339.
    pub wtxidrelay: bool,
    /// The max protocol version supported by both the peer and subcoin.
    pub version: u32,
    /// Peer nonce.
    ///
    /// Used to detect self-connections.
    pub nonce: u64,
    /// Whether the peer prefers the block announcements in `headers`
    /// rather than `inv`.
    pub prefer_headers: bool,
    /// Whether the peer can understand `addrv2` message and prefers to
    /// receive them instead of `addr`.
    pub want_addrv2: bool,
    /// Latency of performed pings.
    pub ping_latency: LatencyTracker,
    /// Current ping state.
    pub ping_state: PingState,
    /// Whether the ping has ever sent to the peer.
    pub has_sent_ping: bool,
    /// Inbound or outbound peer?
    pub direction: Direction,
}

impl PeerInfo {
    fn new(msg: VersionMessage, direction: Direction) -> Self {
        Self {
            best_height: msg.start_height as u32,
            services: msg.services,
            user_agent: msg.user_agent,
            receiver: msg.receiver,
            relay: msg.relay,
            wtxidrelay: false,
            version: msg.version,
            nonce: msg.nonce,
            prefer_headers: false,
            want_addrv2: false,
            ping_latency: LatencyTracker::default(),
            ping_state: PingState::idle(),
            has_sent_ping: false,
            direction,
        }
    }
}

#[derive(Debug)]
pub(crate) struct SlowPeer {
    pub(crate) peer_id: PeerId,
    pub(crate) latency: Latency,
}

/// Manages the peers in the network.
pub struct PeerManager<Block, Client> {
    config: Config,
    client: Arc<Client>,
    address_book: AddressBook,
    handshaking_peers: HashMap<PeerId, HandshakeState>,
    connections: HashMap<PeerId, Connection>,
    connected_peers: HashMap<PeerId, PeerInfo>,
    max_outbound_peers: usize,
    connection_initiator: ConnectionInitiator,
    /// Time at which the slowest peer was evicted.
    last_eviction: Instant,
    rng: fastrand::Rng,
    metrics: Option<Metrics>,
    _phantom: PhantomData<Block>,
}

impl<Block, Client> PeerManager<Block, Client>
where
    Block: BlockT,
    Client: HeaderBackend<Block>,
{
    /// Constructs a new instance of [`PeerManager`].
    pub(crate) fn new(
        client: Arc<Client>,
        config: Config,
        connection_initiator: ConnectionInitiator,
        max_outbound_peers: usize,
        metrics: Option<Metrics>,
    ) -> Self {
        Self {
            config,
            client,
            address_book: AddressBook::new(true, MAX_AVAILABLE_ADDRESSES),
            handshaking_peers: HashMap::new(),
            connections: HashMap::new(),
            connected_peers: HashMap::new(),
            max_outbound_peers,
            connection_initiator,
            last_eviction: Instant::now(),
            rng: fastrand::Rng::new(),
            metrics,
            _phantom: Default::default(),
        }
    }

    pub(crate) fn on_tick(&mut self) -> (Vec<PeerId>, Option<SlowPeer>) {
        let mut timeout_peers = vec![];
        let mut should_ping_peers = vec![];

        for (peer_id, peer) in &self.connected_peers {
            if peer.ping_state.has_timeout() {
                timeout_peers.push(*peer_id);
            } else {
                let should_ping_peer = !peer.has_sent_ping || peer.ping_state.should_ping();
                if should_ping_peer {
                    should_ping_peers.push(*peer_id);
                }
            }
        }

        if !should_ping_peers.is_empty() {
            self.send_pings(should_ping_peers);
        }

        let outbound_peers_count = self
            .connected_peers
            .values()
            .filter(|peer_info| peer_info.direction.is_outbound())
            .count();

        if let Some(metrics) = &self.metrics {
            let inbound_peers_count = self.connected_peers.len() - outbound_peers_count;
            metrics
                .connected_peers
                .with_label_values(&["inbound"])
                .set(inbound_peers_count as u64);
            metrics
                .connected_peers
                .with_label_values(&["outbound"])
                .set(outbound_peers_count as u64);
            metrics
                .addresses
                .with_label_values(&["available"])
                .set(self.address_book.available_addresses_count() as u64);
        }

        let maybe_slow_peer = self.manage_outbound_connections(outbound_peers_count);

        (timeout_peers, maybe_slow_peer)
    }

    /// Manages outbound connections by initiating new connections or evicting slow peers.
    fn manage_outbound_connections(&mut self, outbound_peers_count: usize) -> Option<SlowPeer> {
        if outbound_peers_count < self.max_outbound_peers {
            if let Some(addr) = self.address_book.pop() {
                if !self.connections.contains_key(&addr) {
                    self.connection_initiator.initiate_outbound_connection(addr);
                }
            }
            None
        } else {
            // It's possible for the number of connected peers to temporarily exceed the
            // `max_outbound_peers` limit if multiple connection attempts are in progress
            // and succeed simultaneously. This isn't a significant issue, as the slowest
            // peer can be evicted to enforce the limit.
            //
            // When the `max_inbound_peers` limit is reached, we still attempt to discover
            // potentially better peers by evicting the slowest peer after the eviction
            // interval has elapsed.
            if self.max_outbound_peers > outbound_peers_count
                || self.last_eviction.elapsed() > EVICTION_INTERVAL
            {
                // Find the slowest peer.
                self.connected_peers
                    .iter()
                    .filter_map(|(peer_id, peer_info)| {
                        let avg_latency = peer_info.ping_latency.calc_average()?;

                        if avg_latency > SLOW_PEER_LATENCY {
                            Some((peer_id, avg_latency))
                        } else {
                            None
                        }
                    })
                    .max_by_key(|(_peer_id, avg_latency)| *avg_latency)
                    .map(|(peer_id, latency)| SlowPeer {
                        peer_id: *peer_id,
                        latency,
                    })
            } else {
                None
            }
        }
    }

    fn send_pings(&mut self, should_pings: Vec<PeerId>) {
        for peer_id in should_pings {
            if let Some(peer_info) = self.connected_peers.get_mut(&peer_id) {
                let nonce = self.rng.u64(..);
                peer_info.has_sent_ping = true;
                peer_info.ping_state = PingState::AwaitingPong {
                    last_ping_at: Instant::now(),
                    nonce,
                };
                let _ = self.send(peer_id, NetworkMessage::Ping(nonce));
            }
        }
    }

    /// Sends a network message to the peer.
    pub(crate) fn send(
        &self,
        peer_id: PeerId,
        network_message: NetworkMessage,
    ) -> Result<(), Error> {
        Ok(self
            .connections
            .get(&peer_id)
            .ok_or(Error::ConnectionNotFound(peer_id))?
            .send(network_message)?)
    }

    pub(crate) fn on_outbound_connection_failure(&mut self, addr: PeerId, err: Error) {
        tracing::trace!(?err, "Failed to initiate outbound connection to {addr:?}");

        self.address_book.note_failed_address(addr);

        if let Some(metrics) = &self.metrics {
            metrics
                .addresses
                .with_label_values(&["outbound_connection_failure"])
                .inc();
        }
    }

    /// Disconnects from a specified peer, unless it is designated as persistent, with a given reason.
    ///
    /// # Important Notes
    ///
    /// - **Syncing Components:** This function, as well as [`Self::evict`], should not be invoked
    ///   directly within the peer manager module without triggering a notification. For example,
    ///   chain sync might depend on receiving a disconnect notification to correctly update their
    ///   internal state, which helps maintain a consistent peer set between the peer manager and
    ///   other modules.
    ///
    /// - **Potential for Inconsistent State:** Bypassing notifications may lead to inconsistency
    ///   between the peer manager and modules that rely on peer status, resulting in unexpected
    ///   issues in the peer set or other connected components.
    pub(crate) fn disconnect(&mut self, peer_id: PeerId, reason: Error) {
        if self.config.persistent.contains(&peer_id) {
            return;
        }

        if let Some(connection) = self.connections.remove(&peer_id) {
            tracing::debug!(?reason, "💔 Disconnecting peer {peer_id:?}");
            connection.closer.terminate();

            if let Some(metrics) = &self.metrics {
                metrics.addresses.with_label_values(&["disconnected"]).inc();
            }
        }

        self.address_book.mark_disconnected(&peer_id);
        self.handshaking_peers.remove(&peer_id);
        self.connected_peers.remove(&peer_id);
    }

    /// Evicts a peer, disconnecting it with a specified reason and updating the eviction timestamp.
    ///
    /// This function internally calls [`Self::disconnect`] to carry out the disconnection
    /// process and subsequently records the current time as the `last_eviction` timestamp.
    ///
    /// # Important Note
    ///
    /// Just like with `disconnect`, any call to `evict` should be accompanied by necessary
    /// notifications to avoid state inconsistencies.
    pub(crate) fn evict(&mut self, peer_id: PeerId, reason: Error) {
        self.disconnect(peer_id, reason);
        self.last_eviction = Instant::now();
    }

    /// Sets the prefer addrv2 flag for a peer.
    pub(crate) fn set_want_addrv2(&mut self, peer_id: PeerId) {
        self.connected_peers.entry(peer_id).and_modify(|info| {
            info.want_addrv2 = true;
        });
    }

    /// Sets the prefer headers flag for a peer.
    pub(crate) fn set_prefer_headers(&mut self, peer_id: PeerId) {
        self.connected_peers.entry(peer_id).and_modify(|info| {
            info.prefer_headers = true;
        });
    }

    /// Checks if a peer is connected.
    pub(crate) fn is_connected(&self, peer_id: PeerId) -> bool {
        self.connected_peers.contains_key(&peer_id)
    }

    /// Returns the list of connected peers.
    pub(crate) fn connected_peers(&self) -> impl Iterator<Item = &PeerId> {
        self.connected_peers.keys()
    }

    #[cfg(test)]
    pub(crate) fn local_addr(&self, peer_id: PeerId) -> Option<PeerId> {
        self.connections.get(&peer_id).map(|conn| conn.local_addr)
    }

    /// Returns the number of connected peers.
    pub(crate) fn connected_peers_count(&self) -> usize {
        self.connected_peers.len()
    }

    /// Returns the number of connected peers.
    pub(crate) fn inbound_peers_count(&self) -> usize {
        self.connected_peers
            .values()
            .filter(|peer_info| peer_info.direction.is_inbound())
            .count()
    }

    pub(crate) fn peer_best_number(&self, peer_id: PeerId) -> Option<u32> {
        self.connected_peers
            .get(&peer_id)
            .map(|peer_info| peer_info.best_height)
    }

    /// Handles a new connection.
    pub(crate) fn on_new_connection(&mut self, new_connection: NewConnection) {
        let NewConnection {
            peer_addr,
            local_addr,
            direction,
            writer,
            closer,
        } = new_connection;

        let connection = Connection {
            local_addr,
            writer,
            closer,
        };

        let mut handshake_state = HandshakeState::ConnectionOpened;

        match direction {
            Direction::Inbound => {}
            Direction::Outbound => {
                // Send our version message for the outbound connection.
                let nonce = self.rng.u64(..);
                let local_time = Local::now();
                let best_number = self.client.best_number();

                let version_message = VersionMessage {
                    // Our max supported protocol version.
                    version: self.config.protocol_version,
                    // Local services.
                    services: self.config.services,
                    // Local time.
                    timestamp: local_time.timestamp(),
                    // Receiver address and services, as perceived by us.
                    receiver: Address::new(&peer_addr, ServiceFlags::NONE),
                    // Local address (unreliable) and local services (same as `services` field)
                    sender: Address::new(&connection.local_addr, self.config.services),
                    // A nonce to detect connections to self.
                    nonce,
                    // Our user agent string.
                    user_agent: self.config.user_agent.to_owned(),
                    // Our best height.
                    start_height: best_number as i32,
                    // Whether we want to receive transaction `inv` messages.
                    relay: false,
                };

                if connection
                    .send(NetworkMessage::Version(version_message))
                    .is_err()
                {
                    tracing::debug!(
                        ?peer_addr,
                        "Failed to send version message to peer on new connection"
                    );
                    return;
                }

                let local_time = Local::now();
                handshake_state = HandshakeState::VersionSent { at: local_time };
            }
        }

        self.connections.insert(peer_addr, connection);
        self.handshaking_peers.insert(peer_addr, handshake_state);
    }

    /// Handles receiving a version message.
    pub(crate) fn on_version(
        &mut self,
        peer_id: PeerId,
        direction: Direction,
        version_message: VersionMessage,
    ) -> Result<(), Error> {
        let greatest_common_version = self.config.protocol_version.min(version_message.version);

        tracing::debug!(
            version = version_message.version,
            user_agent = version_message.user_agent,
            start_height = version_message.start_height,
            "Received version from {peer_id:?}"
        );

        match direction {
            Direction::Inbound => {
                let local_addr = self
                    .connections
                    .get(&peer_id)
                    .ok_or(Error::ConnectionNotFound(peer_id))?
                    .local_addr;

                let handshake_state = self
                    .handshaking_peers
                    .get_mut(&peer_id)
                    .ok_or(Error::PeerNotFound(peer_id))?;

                *handshake_state = HandshakeState::VersionReceived(version_message);

                // Send our version.
                let nonce = self.rng.u64(..);
                let best_number = self.client.best_number();
                let local_time = Local::now();
                let our_version = VersionMessage {
                    version: self.config.protocol_version,
                    services: self.config.services,
                    timestamp: local_time.timestamp(),
                    receiver: Address::new(&peer_id, ServiceFlags::NONE),
                    sender: Address::new(&local_addr, self.config.services),
                    nonce,
                    user_agent: self.config.user_agent.to_owned(),
                    start_height: best_number as i32,
                    relay: false,
                };
                self.send(peer_id, NetworkMessage::Version(our_version))?;

                if greatest_common_version >= WTXID_RELAY_VERSION {
                    // TODO: support wtxidrelay
                    // self.send(peer_id, NetworkMessage::WtxidRelay)?;
                }

                // if greatest_common_version >= 70016 {
                // self.send(peer_id, NetworkMessage::SendAddrV2)?;
                // }

                self.send(peer_id, NetworkMessage::Verack)?;
            }
            Direction::Outbound => {
                if version_message.version < MIN_PROTOCOL_VERSION {
                    return Err(Error::ProtocolVersionTooLow);
                }

                // Ensure the peer has required services.
                validate_outbound_services(version_message.services)?;

                match self
                    .handshaking_peers
                    .insert(peer_id, HandshakeState::VersionReceived(version_message))
                {
                    Some(old) => {
                        let HandshakeState::VersionSent { at } = old else {
                            return Err(Error::UnexpectedHandshakeState(Box::new(old)));
                        };

                        if Local::now().signed_duration_since(at).num_seconds() > HANDSHAKE_TIMEOUT
                        {
                            return Err(Error::HandshakeTimeout);
                        }
                    }
                    None => {
                        return Err(Error::PeerNotFound(peer_id));
                    }
                }
            }
        }

        Ok(())
    }

    /// Handles receiving a verack message.
    pub(crate) fn on_verack(&mut self, peer_id: PeerId, direction: Direction) -> Result<(), Error> {
        let Some(handshake_state) = self.handshaking_peers.remove(&peer_id) else {
            return Err(Error::PeerNotFound(peer_id));
        };

        // `verack` must be preceded by `version`.
        let HandshakeState::VersionReceived(version) = handshake_state else {
            return Err(Error::UnexpectedHandshakeState(Box::new(handshake_state)));
        };

        let peer_info = PeerInfo::new(version, direction);

        self.connected_peers.insert(peer_id, peer_info);

        match direction {
            Direction::Inbound => {
                // Do not log the inbound connection success, following Bitcoin Core's behaviour.
                #[cfg(test)]
                tracing::debug!(?direction, "🤝 New peer {peer_id:?}");
            }
            Direction::Outbound => {
                self.send(peer_id, NetworkMessage::Verack)?;
                tracing::debug!(?direction, "🤝 New peer {peer_id:?}");
            }
        }

        if !self.address_book.has_max_addresses() {
            self.send(peer_id, NetworkMessage::GetAddr)?;
        }

        // Immediately ping the peer on ack.
        self.send_pings(vec![peer_id]);

        Ok(())
    }

    pub(crate) fn on_addr(&mut self, peer_id: PeerId, addresses: Vec<(u32, Address)>) {
        let added = self.address_book.add_many(peer_id, addresses);
        if added > 0 {
            tracing::trace!("Added {added} addresses from {peer_id:?}");

            if let Some(metrics) = &self.metrics {
                metrics
                    .addresses
                    .with_label_values(&["discovered"])
                    .add(added as u64);
            }
        }
    }

    pub(crate) fn on_addr_v2(&mut self, peer_id: PeerId, addresses: Vec<AddrV2Message>) {
        let added = self.address_book.add_many_v2(peer_id, addresses);
        if added > 0 {
            tracing::trace!("Added {added} addresses from {peer_id:?}");

            if let Some(metrics) = &self.metrics {
                metrics
                    .addresses
                    .with_label_values(&["discovered"])
                    .add(added as u64);
            }
        }
    }

    pub(crate) fn on_pong(&mut self, peer_id: PeerId, nonce: u64) -> Result<Latency, Error> {
        let peer_info = self
            .connected_peers
            .get_mut(&peer_id)
            .ok_or(Error::PeerNotFound(peer_id))?;

        let latency = match peer_info.ping_state {
            PingState::AwaitingPong {
                last_ping_at,
                nonce: expected,
            } => {
                let got = nonce;
                if got != expected {
                    return Err(Error::BadPong { expected, got });
                }

                let duration = last_ping_at.elapsed();
                if duration >= PingState::PING_TIMEOUT {
                    return Err(Error::PingTimeout);
                }

                duration.as_millis()
            }
            PingState::Idle { .. } => {
                return Err(Error::UnexpectedPong);
            }
        };

        let avg_latency = peer_info.ping_latency.record_pong(latency);
        peer_info.ping_state = PingState::idle();

        tracing::trace!("Received pong from {peer_id} (Avg. Latency: {avg_latency}ms)");

        Ok(avg_latency)
    }
}
