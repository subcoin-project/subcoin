//! UTXO Snapshot Sync (UtxoSnapSync) for fast node bootstrapping.
//!
//! This module implements the client-side P2P protocol for downloading a UTXO set
//! snapshot from peers, enabling new nodes to sync quickly without processing all
//! historical blocks.
//!
//! # Overview
//!
//! `UtxoSnapSync` downloads the complete UTXO set at a specific checkpoint height
//! from peers using a cursor-based pagination protocol. After download, it verifies
//! the data integrity using MuHash (a rolling hash that allows incremental updates).
//!
//! # Protocol Flow
//!
//! ```text
//! Client (Snapcake)                    Server (Full Node)
//!        │                                     │
//!        │──── GetUtxoSetInfo ────────────────>│
//!        │<─── UtxoSetInfo(height, count, mh) ─│
//!        │                                     │
//!        │──── GetUtxoChunk(cursor=None) ─────>│
//!        │<─── UtxoChunk(entries, cursor) ─────│
//!        │                                     │
//!        │──── GetUtxoChunk(cursor=...) ──────>│
//!        │<─── UtxoChunk(entries, cursor) ─────│
//!        │              ...                    │
//!        │──── GetUtxoChunk(cursor=...) ──────>│
//!        │<─── UtxoChunk(entries, complete) ───│
//!        │                                     │
//!        │  [Verify MuHash against checkpoint] │
//!        │                                     │
//! ```
//!
//! # Verification
//!
//! After downloading all UTXOs, the client computes the MuHash of the received
//! data and compares it against the expected checkpoint value. This ensures:
//!
//! - **Data integrity**: No corruption during transfer
//! - **Completeness**: All UTXOs were received
//! - **Correctness**: Data matches Bitcoin Core's UTXO set at that height
//!
//! # Error Handling
//!
//! - **Timeout**: Requests timeout after 30 seconds, peer is deprioritized
//! - **MuHash mismatch**: Storage is cleared and sync restarts with different peer
//! - **Peer failure**: After 3 failures, peer is excluded from future requests

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

/// Strategy key identifying UTXO snap sync in the syncing framework.
pub const UTXO_SNAP_SYNC_KEY: StrategyKey = StrategyKey::new("UtxoSnapSync");

const LOG_TARGET: &str = "sync::utxo";

/// Default number of UTXOs per chunk request.
///
/// 50,000 UTXOs results in approximately 5 MB per chunk, balancing
/// memory usage with network efficiency.
pub const DEFAULT_CHUNK_SIZE: u32 = 50_000;

/// Default maximum number of parallel chunk requests.
///
/// Currently set to 1 because cursor-based pagination requires sequential
/// requests. Future improvements could enable parallel downloads by
/// partitioning the keyspace.
pub const DEFAULT_MAX_PENDING_REQUESTS: usize = 1;

/// Duration before a request is considered timed out.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum request failures before a peer is excluded.
const MAX_PEER_FAILURES: u32 = 3;

/// Progress information for UTXO snap sync.
#[derive(Debug, Clone)]
pub struct UtxoSnapSyncProgress {
    /// Target block height for the UTXO snapshot.
    pub target_height: u32,
    /// Expected MuHash at the target height (from checkpoint).
    pub expected_muhash: String,
    /// Number of UTXOs downloaded so far.
    pub downloaded_utxos: u64,
    /// Whether sync has completed successfully.
    pub is_complete: bool,
    /// Number of chunk requests currently in flight.
    pub pending_requests: usize,
}

/// Internal state for tracking a pending chunk request.
#[derive(Debug)]
struct PendingRequest {
    /// Cursor that was sent with this request.
    cursor: Option<[u8; 36]>,
    /// Timestamp when the request was initiated.
    sent_at: Instant,
}

/// Per-peer tracking state for sync reliability.
#[derive(Debug, Default)]
struct PeerState {
    /// Count of failed requests to this peer.
    failure_count: u32,
    /// Whether we've received UTXO set info from this peer.
    has_utxo_info: bool,
    /// Peer's reported UTXO set height.
    utxo_height: Option<u32>,
    /// Peer's reported UTXO count.
    utxo_count: Option<u64>,
    /// Peer's reported MuHash (as hex string).
    muhash: Option<String>,
}

/// UTXO Snapshot Sync - Downloads UTXO set from peers for fast node bootstrapping.
///
/// This struct implements the client-side logic for the P2P UTXO sync protocol.
/// It manages peer connections, request scheduling, chunk processing, and
/// final verification against MuHash checkpoints.
///
/// # Usage
///
/// ```ignore
/// let snap_sync = UtxoSnapSync::new(
///     840_000,                    // Target height
///     "ba56574e...".to_string(), // Expected MuHash
///     protocol_name,
///     bitcoin_state,
/// );
///
/// // In the sync loop:
/// let actions = snap_sync.actions(&network_service);
/// // ... process actions and responses ...
///
/// if snap_sync.is_complete() {
///     // Sync finished successfully
/// }
/// ```
pub struct UtxoSnapSync<B: BlockT> {
    /// Target block height for the UTXO snapshot.
    target_height: u32,
    /// Expected MuHash at target height (from checkpoint).
    expected_muhash: String,
    /// Protocol name for P2P requests.
    protocol_name: sc_network::ProtocolName,
    /// Bitcoin state storage for importing UTXOs.
    bitcoin_state: Arc<BitcoinState>,
    /// Current pagination cursor (None = start from beginning).
    current_cursor: Option<[u8; 36]>,
    /// Total UTXOs downloaded so far.
    downloaded_utxos: u64,
    /// In-flight requests indexed by peer.
    pending_requests: HashMap<PeerId, PendingRequest>,
    /// Per-peer state tracking.
    peer_states: HashMap<PeerId, PeerState>,
    /// Set of currently connected peers.
    connected_peers: HashSet<PeerId>,
    /// Maximum concurrent requests (currently 1 due to sequential cursors).
    max_pending_requests: usize,
    /// UTXOs to request per chunk.
    chunk_size: u32,
    /// Whether sync completed successfully.
    is_complete: bool,
    /// Whether initial UTXO set info has been requested from peers.
    info_requested: bool,
    /// Phantom for block type parameter.
    _phantom: std::marker::PhantomData<B>,
}

impl<B: BlockT> UtxoSnapSync<B> {
    /// Create a new UTXO snap sync instance.
    ///
    /// # Arguments
    ///
    /// * `target_height` - Block height to sync UTXO set to
    /// * `expected_muhash` - Expected MuHash at target height (from checkpoint)
    /// * `protocol_name` - P2P protocol name for requests
    /// * `bitcoin_state` - Storage backend for importing UTXOs
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
            max_pending_requests: DEFAULT_MAX_PENDING_REQUESTS,
            chunk_size: DEFAULT_CHUNK_SIZE,
            is_complete: false,
            info_requested: false,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Configure maximum number of pending requests.
    ///
    /// Note: Currently only 1 is effective due to sequential cursor pagination.
    pub fn with_max_pending_requests(mut self, max: usize) -> Self {
        self.max_pending_requests = max;
        self
    }

    /// Configure the number of UTXOs to request per chunk.
    pub fn with_chunk_size(mut self, size: u32) -> Self {
        self.chunk_size = size;
        self
    }

    /// Register a newly connected peer.
    pub fn on_peer_connected(&mut self, peer_id: PeerId) {
        self.connected_peers.insert(peer_id);
        self.peer_states.entry(peer_id).or_default();
    }

    /// Handle peer disconnection.
    pub fn on_peer_disconnected(&mut self, peer_id: &PeerId) {
        self.connected_peers.remove(peer_id);
        self.pending_requests.remove(peer_id);
    }

    /// Process UTXO set info response from a peer.
    ///
    /// This validates that the peer has data at our target height and
    /// that their MuHash matches our expected checkpoint.
    pub fn on_utxo_set_info(
        &mut self,
        peer_id: PeerId,
        height: u32,
        utxo_count: u64,
        muhash: v1::MuHashCommitment,
    ) {
        let muhash_hex = muhash.txoutset_muhash();

        tracing::info!(
            target: LOG_TARGET,
            "Peer {peer_id} UTXO set: height={height}, count={utxo_count}, muhash={muhash_hex}"
        );

        if let Some(state) = self.peer_states.get_mut(&peer_id) {
            state.has_utxo_info = true;
            state.utxo_height = Some(height);
            state.utxo_count = Some(utxo_count);
            state.muhash = Some(muhash_hex.clone());
        }

        // Validate peer can serve our target height
        if height < self.target_height {
            tracing::warn!(
                target: LOG_TARGET,
                "Peer {peer_id} height {height} < target {}",
                self.target_height
            );
            return;
        }

        // Validate MuHash if peer is at our exact target height
        if height == self.target_height && muhash_hex != self.expected_muhash {
            tracing::warn!(
                target: LOG_TARGET,
                "Peer {peer_id} MuHash mismatch: expected {}, got {muhash_hex}",
                self.expected_muhash
            );
            self.record_peer_failure(&peer_id);
        }
    }

    /// Process a UTXO chunk response from a peer.
    ///
    /// This imports the received UTXOs into storage and updates the cursor
    /// for the next request. When the final chunk is received, it verifies
    /// the complete UTXO set against the expected MuHash.
    pub fn on_utxo_chunk(
        &mut self,
        peer_id: PeerId,
        entries: Vec<v1::UtxoEntry>,
        next_cursor: Option<[u8; 36]>,
        is_complete: bool,
    ) -> Result<(), String> {
        self.pending_requests.remove(&peer_id);

        let chunk_size = entries.len();
        tracing::debug!(
            target: LOG_TARGET,
            "Received {chunk_size} UTXOs from {peer_id}, complete={is_complete}"
        );

        // Convert protocol entries to storage format
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

        // Import into storage
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
            "Imported {imported} UTXOs, total: {}",
            self.downloaded_utxos
        );

        if is_complete {
            self.finalize_sync(peer_id)?;
        } else {
            self.current_cursor = next_cursor;
        }

        Ok(())
    }

    /// Finalize sync after receiving all chunks.
    fn finalize_sync(&mut self, peer_id: PeerId) -> Result<(), String> {
        tracing::info!(
            target: LOG_TARGET,
            "UTXO download complete: {} UTXOs",
            self.downloaded_utxos
        );

        // Finalize the import (sets height and computes final MuHash)
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

        // Verify MuHash matches checkpoint
        if !self.bitcoin_state.verify_muhash(&self.expected_muhash) {
            let actual = self.bitcoin_state.muhash_hex();
            tracing::warn!(
                target: LOG_TARGET,
                "MuHash verification failed: expected {}, got {actual}",
                self.expected_muhash
            );

            self.record_peer_failure(&peer_id);
            self.reset_for_retry()
                .map_err(|e| format!("Failed to reset: {e}"))?;

            return Ok(());
        }

        self.is_complete = true;
        tracing::info!(target: LOG_TARGET, "UTXO sync verified successfully!");
        Ok(())
    }

    /// Reset state to retry sync after verification failure.
    fn reset_for_retry(&mut self) -> Result<(), String> {
        self.bitcoin_state
            .clear()
            .map_err(|e| format!("Failed to clear storage: {e}"))?;

        self.current_cursor = None;
        self.downloaded_utxos = 0;
        self.info_requested = false;

        tracing::info!(target: LOG_TARGET, "Reset sync state for retry");
        Ok(())
    }

    /// Record a failure for a peer, potentially excluding them from future requests.
    fn record_peer_failure(&mut self, peer_id: &PeerId) {
        if let Some(state) = self.peer_states.get_mut(peer_id) {
            state.failure_count = state.failure_count.saturating_add(1);
            if state.failure_count >= MAX_PEER_FAILURES {
                tracing::warn!(
                    target: LOG_TARGET,
                    "Peer {peer_id} excluded after {MAX_PEER_FAILURES} failures"
                );
            }
        }
    }

    /// Check for and handle timed-out requests.
    fn process_timeouts(&mut self) {
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
            self.record_peer_failure(&peer_id);
        }
    }

    /// Get peers suitable for sending requests to.
    fn suitable_peers(&self) -> Vec<PeerId> {
        self.connected_peers
            .iter()
            .filter(|peer_id| {
                // Skip peers with pending requests
                if self.pending_requests.contains_key(peer_id) {
                    return false;
                }

                // Skip peers that have failed too many times
                if let Some(state) = self.peer_states.get(peer_id) {
                    if state.failure_count >= MAX_PEER_FAILURES {
                        return false;
                    }

                    // Prefer peers with confirmed UTXO info at sufficient height
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

    /// Generate sync actions for the network layer.
    ///
    /// This is called by the syncing framework to get pending network requests.
    pub fn actions(&mut self, network_service: &NetworkServiceHandle) -> Vec<SyncingAction<B>> {
        if self.is_complete {
            return Vec::new();
        }

        self.process_timeouts();

        let mut actions = Vec::new();

        // First phase: request UTXO set info from all peers
        if !self.info_requested {
            for peer_id in self.connected_peers.iter().copied() {
                let request = VersionedNetworkRequest::<B>::V1(v1::NetworkRequest::GetUtxoSetInfo);
                actions.push(self.create_request_action(peer_id, request, network_service));
            }
            self.info_requested = true;
            return actions;
        }

        // Check if we can send more requests
        let available_slots = self
            .max_pending_requests
            .saturating_sub(self.pending_requests.len());
        if available_slots == 0 {
            return actions;
        }

        // Send chunk request to a suitable peer
        // (Sequential cursor means only one active request at a time)
        if let Some(peer_id) = self.suitable_peers().into_iter().next() {
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

    /// Create a network request action for the syncing framework.
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
            key: UTXO_SNAP_SYNC_KEY,
            request: async move {
                Ok(rx.await?.map(|(response, protocol_name)| {
                    (Box::new(response) as Box<dyn Any + Send>, protocol_name)
                }))
            }
            .boxed(),
            remove_obsolete: false,
        }
    }

    /// Check if sync has completed successfully.
    pub fn is_complete(&self) -> bool {
        self.is_complete
    }

    /// Get current sync progress.
    pub fn progress(&self) -> UtxoSnapSyncProgress {
        UtxoSnapSyncProgress {
            target_height: self.target_height,
            expected_muhash: self.expected_muhash.clone(),
            downloaded_utxos: self.downloaded_utxos,
            is_complete: self.is_complete,
            pending_requests: self.pending_requests.len(),
        }
    }
}
