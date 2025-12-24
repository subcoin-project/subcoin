//! UTXO sync strategy for fast synchronization.
//!
//! This module implements the client-side P2P UTXO sync protocol that downloads
//! UTXO chunks from peers and verifies them using MuHash.

use bitcoin::hashes::Hash;
use codec::Encode;
use futures::FutureExt;
use sc_network::PeerId;
use sc_network_sync::service::network::NetworkServiceHandle;
use sc_network_sync::strategy::{StrategyKey, SyncingAction};
use sp_runtime::traits::Block as BlockT;
use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use subcoin_bitcoin_state::BitcoinState;
use subcoin_service::network_request_handler::{VersionedNetworkRequest, v1};

/// Strategy key for UTXO sync.
pub const UTXO_SYNC_STRATEGY_KEY: StrategyKey = StrategyKey::new("UtxoSync");

const LOG_TARGET: &str = "sync::utxo";

/// Default number of UTXOs per chunk.
pub const DEFAULT_CHUNK_SIZE: u32 = 50_000;

/// Maximum number of parallel chunk requests.
pub const DEFAULT_PARALLEL_REQUESTS: usize = 4;

/// Request timeout duration.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum retries per peer before marking it as failed.
const MAX_PEER_RETRIES: u32 = 3;

/// Progress of UTXO sync.
#[derive(Debug, Clone)]
pub struct UtxoSyncProgress {
    /// Target height for fast sync.
    pub target_height: u32,
    /// Expected MuHash at target height.
    pub expected_muhash: String,
    /// Number of UTXOs downloaded so far.
    pub downloaded_utxos: u64,
    /// Whether sync is complete.
    pub is_complete: bool,
    /// Number of pending requests.
    pub pending_requests: usize,
}

/// State of a pending chunk request.
#[derive(Debug)]
struct PendingRequest {
    /// The cursor for this request.
    cursor: Option<[u8; 36]>,
    /// Time when the request was sent.
    sent_at: Instant,
}

/// Peer state for UTXO sync.
#[derive(Debug, Default)]
struct PeerState {
    /// Number of failed requests to this peer.
    failed_requests: u32,
    /// Whether this peer has UTXO set info.
    has_utxo_info: bool,
    /// Peer's reported UTXO height.
    utxo_height: Option<u32>,
    /// Peer's reported UTXO count.
    utxo_count: Option<u64>,
    /// Peer's reported MuHash.
    muhash: Option<String>,
}

/// UTXO sync strategy for downloading UTXOs from peers.
pub struct UtxoSyncStrategy<B: BlockT> {
    /// Target height for fast sync.
    target_height: u32,
    /// Expected MuHash at target height.
    expected_muhash: String,
    /// Protocol name for requests.
    protocol_name: sc_network::ProtocolName,
    /// Bitcoin state storage.
    bitcoin_state: Arc<BitcoinState>,
    /// Current cursor for pagination.
    current_cursor: Option<[u8; 36]>,
    /// Number of UTXOs downloaded.
    downloaded_utxos: u64,
    /// Pending chunk requests by peer.
    pending_requests: HashMap<PeerId, PendingRequest>,
    /// Peer states.
    peer_states: HashMap<PeerId, PeerState>,
    /// Connected peers.
    connected_peers: HashSet<PeerId>,
    /// Maximum parallel requests.
    max_parallel_requests: usize,
    /// Chunk size (UTXOs per chunk).
    chunk_size: u32,
    /// Whether sync is complete.
    is_complete: bool,
    /// Whether we've requested UTXO set info from peers.
    info_requested: bool,
    /// Phantom for block type.
    _phantom: std::marker::PhantomData<B>,
}

impl<B: BlockT> UtxoSyncStrategy<B> {
    /// Create a new UTXO sync strategy.
    pub fn new(
        target_height: u32,
        expected_muhash: String,
        protocol_name: sc_network::ProtocolName,
        bitcoin_state: Arc<BitcoinState>,
    ) -> Self {
        Self {
            target_height,
            expected_muhash,
            protocol_name,
            bitcoin_state,
            current_cursor: None,
            downloaded_utxos: 0,
            pending_requests: HashMap::new(),
            peer_states: HashMap::new(),
            connected_peers: HashSet::new(),
            max_parallel_requests: DEFAULT_PARALLEL_REQUESTS,
            chunk_size: DEFAULT_CHUNK_SIZE,
            is_complete: false,
            info_requested: false,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Configure maximum parallel requests.
    pub fn with_max_parallel_requests(mut self, max: usize) -> Self {
        self.max_parallel_requests = max;
        self
    }

    /// Configure chunk size.
    pub fn with_chunk_size(mut self, size: u32) -> Self {
        self.chunk_size = size;
        self
    }

    /// Register a new peer.
    pub fn on_peer_connected(&mut self, peer_id: PeerId) {
        self.connected_peers.insert(peer_id);
        self.peer_states.entry(peer_id).or_default();
    }

    /// Handle peer disconnection.
    pub fn on_peer_disconnected(&mut self, peer_id: &PeerId) {
        self.connected_peers.remove(peer_id);
        self.pending_requests.remove(peer_id);
    }

    /// Handle UTXO set info response from a peer.
    pub fn on_utxo_set_info(
        &mut self,
        peer_id: PeerId,
        height: u32,
        utxo_count: u64,
        muhash: v1::MuHashCommitment,
    ) {
        // Compute the txoutset muhash (32-byte hash) for comparison with checkpoints
        let muhash_hex = muhash.txoutset_muhash();

        tracing::info!(
            target: LOG_TARGET,
            "Peer {peer_id} UTXO set info: height={height}, count={utxo_count}, muhash={muhash_hex}"
        );

        if let Some(state) = self.peer_states.get_mut(&peer_id) {
            state.has_utxo_info = true;
            state.utxo_height = Some(height);
            state.utxo_count = Some(utxo_count);
            state.muhash = Some(muhash_hex.clone());
        }

        // Validate peer has the data we need
        if height < self.target_height {
            tracing::warn!(
                target: LOG_TARGET,
                "Peer {peer_id} height {height} is below target {}", self.target_height
            );
            return;
        }

        // Check if MuHash matches (if we know the expected value)
        if height == self.target_height && muhash_hex != self.expected_muhash {
            tracing::warn!(
                target: LOG_TARGET,
                "Peer {peer_id} MuHash mismatch: expected {}, got {muhash_hex}",
                self.expected_muhash
            );
            self.mark_peer_failed(&peer_id);
        }
    }

    /// Handle UTXO chunk response from a peer.
    pub fn on_utxo_chunk(
        &mut self,
        peer_id: PeerId,
        entries: Vec<v1::UtxoEntry>,
        next_cursor: Option<[u8; 36]>,
        is_complete: bool,
    ) -> Result<(), String> {
        // Remove the pending request
        let _pending = self.pending_requests.remove(&peer_id);

        let entries_count = entries.len();
        tracing::debug!(
            target: LOG_TARGET,
            "Received {entries_count} UTXOs from {peer_id}, complete={is_complete}"
        );

        // Convert entries to (OutPoint, Coin) pairs
        let utxos: Vec<(bitcoin::OutPoint, subcoin_bitcoin_state::Coin)> = entries
            .into_iter()
            .map(|entry| {
                let txid = bitcoin::Txid::from_byte_array(entry.txid);
                let outpoint = bitcoin::OutPoint::new(txid, entry.vout);
                let coin = subcoin_bitcoin_state::Coin {
                    is_coinbase: entry.is_coinbase,
                    amount: entry.amount,
                    height: entry.height,
                    script_pubkey: entry.script_pubkey,
                };
                (outpoint, coin)
            })
            .collect();

        // Import the UTXOs
        let imported = self
            .bitcoin_state
            .bulk_import(utxos)
            .map_err(|e| format!("Failed to import UTXOs: {e}"))?;

        self.downloaded_utxos = self
            .downloaded_utxos
            .checked_add(imported as u64)
            .ok_or("UTXO count overflow")?;

        tracing::info!(
            target: LOG_TARGET,
            "Imported {imported} UTXOs, total: {}", self.downloaded_utxos
        );

        // Update cursor for next request
        if is_complete {
            tracing::info!(
                target: LOG_TARGET,
                "UTXO sync complete: {} UTXOs downloaded",
                self.downloaded_utxos
            );

            // Finalize the import
            self.bitcoin_state
                .finalize_import(self.target_height)
                .map_err(|e| format!("Failed to finalize import: {e}"))?;

            tracing::info!(
                target: LOG_TARGET,
                "After finalize: height={}, utxo_count={}, muhash={}",
                self.bitcoin_state.height(),
                self.bitcoin_state.utxo_count(),
                self.bitcoin_state.muhash_hex()
            );

            // Verify final MuHash
            if !self.bitcoin_state.verify_muhash(&self.expected_muhash) {
                let actual = self.bitcoin_state.muhash_hex();

                // MuHash verification failed - reset state and try again with different peer
                tracing::warn!(
                    target: LOG_TARGET,
                    "MuHash mismatch: expected {}, got {actual}. Clearing storage and retrying.",
                    self.expected_muhash
                );

                // Mark the peer that sent bad data as failed
                self.mark_peer_failed(&peer_id);

                // Clear storage and reset sync state
                self.reset_for_retry()
                    .map_err(|e| format!("Failed to reset for retry: {e}"))?;

                return Ok(()); // Continue with retry, don't propagate error
            }

            self.is_complete = true;
            tracing::info!(target: LOG_TARGET, "UTXO sync verified successfully!");
        } else {
            self.current_cursor = next_cursor;
        }

        Ok(())
    }

    /// Reset sync state for retry after verification failure.
    fn reset_for_retry(&mut self) -> Result<(), String> {
        // Clear storage
        self.bitcoin_state
            .clear()
            .map_err(|e| format!("Failed to clear storage: {e}"))?;

        // Reset sync state
        self.current_cursor = None;
        self.downloaded_utxos = 0;
        self.info_requested = false; // Re-request info from peers

        tracing::info!(target: LOG_TARGET, "Reset sync state for retry");
        Ok(())
    }

    /// Mark a peer as failed.
    fn mark_peer_failed(&mut self, peer_id: &PeerId) {
        if let Some(state) = self.peer_states.get_mut(peer_id) {
            state.failed_requests = state.failed_requests.saturating_add(1);
            if state.failed_requests >= MAX_PEER_RETRIES {
                tracing::warn!(
                    target: LOG_TARGET,
                    "Peer {peer_id} exceeded max retries, marking as failed"
                );
            }
        }
    }

    /// Check for timed-out requests.
    fn check_timeouts(&mut self) {
        let now = Instant::now();
        let timed_out: Vec<PeerId> = self
            .pending_requests
            .iter()
            .filter(|(_, req)| now.duration_since(req.sent_at) > REQUEST_TIMEOUT)
            .map(|(peer_id, _)| *peer_id)
            .collect();

        for peer_id in timed_out {
            tracing::warn!(target: LOG_TARGET, "Request to {peer_id} timed out");
            self.pending_requests.remove(&peer_id);
            self.mark_peer_failed(&peer_id);
        }
    }

    /// Get peers that are suitable for requests.
    fn get_suitable_peers(&self) -> Vec<PeerId> {
        self.connected_peers
            .iter()
            .filter(|peer_id| {
                // Skip peers with pending requests
                if self.pending_requests.contains_key(peer_id) {
                    return false;
                }
                // Skip failed peers
                if let Some(state) = self.peer_states.get(peer_id) {
                    if state.failed_requests >= MAX_PEER_RETRIES {
                        return false;
                    }
                    // Prefer peers that have confirmed UTXO info
                    if state.has_utxo_info {
                        if let Some(height) = state.utxo_height {
                            return height >= self.target_height;
                        }
                    }
                }
                true
            })
            .copied()
            .collect()
    }

    /// Generate sync actions.
    pub fn actions(&mut self, network_service: &NetworkServiceHandle) -> Vec<SyncingAction<B>> {
        if self.is_complete {
            return Vec::new();
        }

        self.check_timeouts();

        let mut actions = Vec::new();

        // First, request UTXO set info from peers if we haven't yet
        if !self.info_requested {
            for peer_id in self.connected_peers.iter().copied() {
                let request = VersionedNetworkRequest::<B>::V1(v1::NetworkRequest::GetUtxoSetInfo);
                actions.push(self.create_request_action(peer_id, request, network_service));
            }
            self.info_requested = true;
            return actions;
        }

        // Calculate how many requests we can make
        let available_slots = self
            .max_parallel_requests
            .saturating_sub(self.pending_requests.len());
        if available_slots == 0 {
            return actions;
        }

        // Get suitable peers and make one chunk request
        // (sequential cursor means only one request at a time)
        let suitable_peers = self.get_suitable_peers();
        if let Some(peer_id) = suitable_peers.into_iter().next() {
            let request = VersionedNetworkRequest::<B>::V1(v1::NetworkRequest::GetUtxoChunk {
                cursor: self.current_cursor,
                max_entries: self.chunk_size,
            });

            self.pending_requests.insert(
                peer_id,
                PendingRequest {
                    cursor: self.current_cursor,
                    sent_at: Instant::now(),
                },
            );

            actions.push(self.create_request_action(peer_id, request, network_service));
        }

        actions
    }

    /// Create a network request action.
    fn create_request_action(
        &self,
        peer_id: PeerId,
        request: VersionedNetworkRequest<B>,
        network_service: &NetworkServiceHandle,
    ) -> SyncingAction<B> {
        let (tx, rx) = futures::channel::oneshot::channel();

        network_service.start_request(
            peer_id,
            self.protocol_name.clone(),
            request.encode(),
            tx,
            sc_network::IfDisconnected::ImmediateError,
        );

        SyncingAction::StartRequest {
            peer_id,
            key: UTXO_SYNC_STRATEGY_KEY,
            request: async move {
                Ok(rx.await?.map(|(response, protocol_name)| {
                    (Box::new(response) as Box<dyn Any + Send>, protocol_name)
                }))
            }
            .boxed(),
            remove_obsolete: false,
        }
    }

    /// Check if sync is complete.
    pub fn is_complete(&self) -> bool {
        self.is_complete
    }

    /// Get current progress.
    pub fn progress(&self) -> UtxoSyncProgress {
        UtxoSyncProgress {
            target_height: self.target_height,
            expected_muhash: self.expected_muhash.clone(),
            downloaded_utxos: self.downloaded_utxos,
            is_complete: self.is_complete,
            pending_requests: self.pending_requests.len(),
        }
    }
}
