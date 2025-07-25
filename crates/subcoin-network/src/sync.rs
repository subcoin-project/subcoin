mod block_downloader;
mod orphan_blocks_pool;
mod strategy;

use self::strategy::{BlocksFirstStrategy, HeadersFirstStrategy};
use crate::peer_manager::NewPeer;
use crate::peer_store::PeerStore;
use crate::{Error, Latency, PeerId, SyncStatus, SyncStrategy};
use bitcoin::blockdata::block::Header as BitcoinHeader;
use bitcoin::p2p::message_blockdata::Inventory;
use bitcoin::{Block as BitcoinBlock, BlockHash};
use sc_client_api::{AuxStore, HeaderBackend};
use sc_consensus_nakamoto::{
    BlockImportQueue, HeaderVerifier, ImportBlocks, ImportManyBlocksResult,
};
use serde::{Deserialize, Serialize};
use sp_consensus::BlockOrigin;
use sp_runtime::traits::Block as BlockT;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use subcoin_primitives::ClientExt;

// Do major sync when the current tip falls behind the network by 144 blocks (roughly one day).
const MAJOR_SYNC_GAP: u32 = 144;

const LATENCY_IMPROVEMENT_THRESHOLD: u128 = 4;

// Define a constant for the low ping latency cutoff, in milliseconds.
const LOW_LATENCY_CUTOFF: Latency = 20;

/// Maximum number of syncing retries for a deprioritized peer.
const MAX_STALLS: usize = 5;

/// The state of syncing between a Peer and ourselves.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize, Hash)]
#[serde(rename_all = "camelCase")]
pub enum PeerSyncState {
    /// Available for sync requests.
    Available,
    /// The peer has been deprioritized due to past syncing issues (e.g., stalling).
    Deprioritized {
        /// Number of times the peer has stalled.
        stalled_count: usize,
    },
    /// Actively downloading new blocks, starting from the given Number.
    DownloadingNew { start: u32 },
}

impl PeerSyncState {
    /// Returns `true` if the peer is available for syncing.
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available)
    }

    fn stalled_count(&self) -> usize {
        match self {
            Self::Deprioritized { stalled_count } => *stalled_count,
            _ => 0,
        }
    }

    /// Determines if the peer is permanently deprioritized based on the stall count.
    fn is_permanently_deprioritized(&self) -> bool {
        match self {
            PeerSyncState::Deprioritized { stalled_count } => *stalled_count > MAX_STALLS,
            _ => false,
        }
    }
}

/// Contains all the data about a Peer that we are trying to sync with.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerSync {
    /// Peer id of this peer.
    pub peer_id: PeerId,
    /// The number of the best block that we've seen for this peer.
    pub best_number: u32,
    /// Latency of connection to this peer.
    pub latency: Latency,
    /// The state of syncing this peer is in for us, generally categories
    /// into `Available` or "busy" with something as defined by `PeerSyncState`.
    pub state: PeerSyncState,
}

/// Locator based sync request, for requesting either Headers or Blocks.
#[derive(Debug, PartialEq, Eq)]
pub struct LocatorRequest {
    pub locator_hashes: Vec<BlockHash>,
    pub stop_hash: BlockHash,
    pub to: PeerId,
}

/// Represents different kinds of sync requests.
#[derive(Debug, PartialEq, Eq)]
pub enum SyncRequest {
    /// Request headers via `getheaders`.
    Header(LocatorRequest),
    /// Request inventories via `getblocks`.
    Inventory(LocatorRequest),
    /// Request blocks via `getdata`.
    Data(Vec<Inventory>, PeerId),
}

/// Represents actions that can be taken during the syncing.
#[derive(Debug)]
pub enum SyncAction {
    /// Fetch headers, blocks and data.
    Request(SyncRequest),
    /// Transitions to a Blocks-First sync after Headers-First sync
    /// compltes, to fetch the most recent blocks.
    SwitchToBlocksFirstSync,
    /// Disconnect from the peer for the given reason.
    DisconnectPeer(PeerId, Error),
    /// Deprioritize the specified peer, restarting the current sync
    /// with other candidates if available.
    RestartSyncWithStalledPeer(PeerId),
    /// Blocks-First sync finished and sets the syncing state to idle.
    SetIdle,
    /// No action needed.
    None,
}

impl SyncAction {
    pub(crate) fn get_headers(request: LocatorRequest) -> Self {
        Self::Request(SyncRequest::Header(request))
    }

    pub(crate) fn get_inventory(request: LocatorRequest) -> Self {
        Self::Request(SyncRequest::Inventory(request))
    }

    pub(crate) fn get_data(inv: Vec<Inventory>, from: PeerId) -> Self {
        Self::Request(SyncRequest::Data(inv, from))
    }
}

#[derive(Debug)]
pub(crate) enum RestartReason {
    Stalled,
    Disconnected,
}

// This enum encapsulates the various strategies and states a node
// might be in during the sync process.
enum Syncing<Block, Client> {
    /// Blocks-First sync.
    BlocksFirst(Box<BlocksFirstStrategy<Block, Client>>),
    /// Headers-First sync.
    HeadersFirst(Box<HeadersFirstStrategy<Block, Client>>),
    /// Not syncing.
    ///
    /// This could indicate that the node is either fully synced
    /// or is waiting for more peers to resume syncing.
    Idle,
}

impl<Block, Client> Syncing<Block, Client> {
    fn is_major_syncing(&self) -> bool {
        matches!(self, Self::BlocksFirst(_) | Self::HeadersFirst(_))
    }
}

/// The main data structure which contains all the state for syncing.
pub(crate) struct ChainSync<Block, Client> {
    /// Chain client.
    client: Arc<Client>,
    /// Block header verifier.
    header_verifier: HeaderVerifier<Block, Client>,
    /// The active peers that we are using to sync and their PeerSync status
    pub(crate) peers: HashMap<PeerId, PeerSync>,
    /// Current syncing state.
    syncing: Syncing<Block, Client>,
    /// Handle of the import queue.
    pub(crate) import_queue: BlockImportQueue,
    /// Block syncing strategy.
    sync_strategy: SyncStrategy,
    /// Are we in major syncing?
    is_major_syncing: Arc<AtomicBool>,
    /// Whether to sync blocks from Bitcoin network.
    enable_block_sync: bool,
    /// Randomness generator.
    rng: fastrand::Rng,
    /// Handle of peer store.
    peer_store: Arc<dyn PeerStore>,
    /// Target block of the syncing process.
    sync_target: Option<u32>,
    min_sync_peer_threshold: usize,
    _phantom: PhantomData<Block>,
}

impl<Block, Client> ChainSync<Block, Client>
where
    Block: BlockT,
    Client: HeaderBackend<Block> + AuxStore,
{
    /// Constructs a new instance of [`ChainSync`].
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        client: Arc<Client>,
        header_verifier: HeaderVerifier<Block, Client>,
        import_queue: BlockImportQueue,
        sync_strategy: SyncStrategy,
        is_major_syncing: Arc<AtomicBool>,
        enable_block_sync: bool,
        peer_store: Arc<dyn PeerStore>,
        sync_target: Option<u32>,
        min_sync_peer_threshold: usize,
    ) -> Self {
        Self {
            client,
            header_verifier,
            peers: HashMap::new(),
            syncing: Syncing::Idle,
            import_queue,
            sync_strategy,
            is_major_syncing,
            enable_block_sync,
            rng: fastrand::Rng::new(),
            peer_store,
            sync_target,
            min_sync_peer_threshold,
            _phantom: Default::default(),
        }
    }

    pub(super) fn sync_status(&self) -> SyncStatus {
        match &self.syncing {
            Syncing::Idle => SyncStatus::Idle,
            Syncing::BlocksFirst(strategy) => strategy.sync_status(),
            Syncing::HeadersFirst(strategy) => strategy.sync_status(),
        }
    }

    pub(super) fn is_idle(&self) -> bool {
        matches!(self.syncing, Syncing::Idle)
    }

    pub(super) fn on_tick(&mut self) -> SyncAction {
        match &mut self.syncing {
            Syncing::Idle => SyncAction::None,
            Syncing::BlocksFirst(strategy) => strategy.on_tick(),
            Syncing::HeadersFirst(strategy) => strategy.on_tick(),
        }
    }

    pub(super) async fn wait_for_block_import_results(&mut self) -> ImportManyBlocksResult {
        self.import_queue.block_import_results().await
    }

    pub(super) fn unreliable_peers(&self) -> Vec<PeerId> {
        self.peers
            .iter()
            .filter_map(|(peer_id, peer)| {
                peer.state
                    .is_permanently_deprioritized()
                    .then_some(peer_id)
                    .copied()
            })
            .collect()
    }

    /// Removes the given peer from peers of chain sync.
    pub(super) fn disconnect(&mut self, peer_id: PeerId) {
        if let Some(removed_peer) = self.peers.remove(&peer_id) {
            // We currently support only one syncing peer, this logic needs to be
            // refactored once multiple syncing peers are supported.
            if matches!(removed_peer.state, PeerSyncState::DownloadingNew { .. }) {
                self.restart_sync(removed_peer.peer_id, RestartReason::Disconnected);
            }
        }
    }

    pub(super) fn update_peer_best(&mut self, peer_id: PeerId, peer_best: u32) {
        let mut peer_best_updated = false;

        self.peers.entry(peer_id).and_modify(|e| {
            if peer_best > e.best_number {
                tracing::debug!(
                    "Tip of {peer_id:?} updated from #{} to #{peer_best}",
                    e.best_number
                );
                e.best_number = peer_best;
                peer_best_updated = true;
            }
        });

        if peer_best_updated {
            match &mut self.syncing {
                Syncing::Idle => {}
                Syncing::BlocksFirst(strategy) => strategy.set_peer_best(peer_id, peer_best),
                Syncing::HeadersFirst(strategy) => strategy.set_peer_best(peer_id, peer_best),
            }
        }
    }

    /// Attempt to find the best available peer, falling back to a random choice if needed
    fn select_next_peer_for_sync(
        &mut self,
        our_best: u32,
        excluded_peer: PeerId,
    ) -> Option<PeerId> {
        self.peers
            .values()
            .filter(|peer| {
                peer.peer_id != excluded_peer
                    && peer.best_number > our_best
                    && peer.state.is_available()
            })
            .min_by_key(|peer| peer.latency)
            .map(|peer| peer.peer_id)
            .or_else(|| {
                let sync_candidates = self
                    .peers
                    .values()
                    .filter(|peer| peer.peer_id != excluded_peer && peer.best_number > our_best)
                    .collect::<Vec<_>>();

                // Pick a random peer, even if it's marked as deprioritized.
                self.rng.choice(sync_candidates).map(|peer| peer.peer_id)
            })
    }

    /// Attempts to restart the sync based on the reason provided.
    ///
    /// Returns `true` if the sync is restarted with a new peer.
    pub(super) fn restart_sync(&mut self, prior_peer_id: PeerId, reason: RestartReason) {
        let our_best = self.client.best_number();

        let Some(new_peer_id) = self.select_next_peer_for_sync(our_best, prior_peer_id) else {
            if let Some(median_seen_block) = self.median_seen() {
                if median_seen_block <= our_best {
                    let best_seen_block = self.peers.values().map(|p| p.best_number).max();

                    // We are synced to the median block seen by our peers, but this may
                    // not be the network's tip.
                    //
                    // Transition to idle unless more blocks are announced.
                    tracing::debug!(
                        best_seen_block,
                        median_seen_block,
                        our_best,
                        "Synced to the majority of peers, switching to Idle"
                    );
                    self.update_syncing_state(Syncing::Idle);
                    return;
                }
            }

            // No new sync candidate, keep it as is.
            // TODO: handle this properly.
            tracing::debug!(
                ?prior_peer_id,
                "⚠️ Attempting to restart sync, but no new sync candidate available"
            );

            return;
        };

        {
            let Some(new_peer) = self.peers.get_mut(&new_peer_id) else {
                tracing::error!("Corrupted state, next peer {new_peer_id} missing from peer list");
                return;
            };

            tracing::debug!(?reason, prior_peer = ?prior_peer_id, ?new_peer, "🔄 Sync restarted");
            new_peer.state = PeerSyncState::DownloadingNew { start: our_best };

            let target_block_number = target_block_number(self.sync_target, new_peer.best_number);

            match &mut self.syncing {
                Syncing::BlocksFirst(strategy) => {
                    strategy.restart(new_peer.peer_id, target_block_number);
                }
                Syncing::HeadersFirst(strategy) => {
                    strategy.restart(new_peer.peer_id, target_block_number);
                }
                Syncing::Idle => {}
            }
        }

        match reason {
            RestartReason::Stalled => {
                self.peers.entry(prior_peer_id).and_modify(|p| {
                    let current_stalled_count = p.state.stalled_count();
                    p.state = PeerSyncState::Deprioritized {
                        stalled_count: current_stalled_count + 1,
                    };
                });
            }
            RestartReason::Disconnected => {
                // Nothing to be done, peer is already removed from the peer list.
            }
        }
    }

    /// Returns the median block number advertised by our peers.
    fn median_seen(&self) -> Option<u32> {
        let mut best_seens = self
            .peers
            .values()
            .map(|p| p.best_number)
            .collect::<Vec<_>>();

        if best_seens.is_empty() {
            None
        } else {
            let middle = best_seens.len() / 2;

            Some(*best_seens.select_nth_unstable(middle).1)
        }
    }

    pub(super) fn update_peer_latency(&mut self, peer_id: PeerId, avg_latency: Latency) {
        self.peers.entry(peer_id).and_modify(|peer| {
            peer.latency = avg_latency;
        });
        self.update_sync_peer_on_lower_latency();
    }

    pub(super) fn update_sync_peer_on_lower_latency(&mut self) {
        let maybe_sync_peer_id = match &self.syncing {
            Syncing::Idle => return,
            Syncing::BlocksFirst(strategy) => strategy.can_swap_sync_peer(),
            Syncing::HeadersFirst(strategy) => strategy.can_swap_sync_peer(),
        };

        let Some(current_sync_peer_id) = maybe_sync_peer_id else {
            return;
        };

        let Some(current_latency) = self
            .peers
            .get(&current_sync_peer_id)
            .map(|peer| peer.latency)
        else {
            return;
        };

        if current_latency <= LOW_LATENCY_CUTOFF {
            tracing::trace!(
                peer_id = ?current_sync_peer_id,
                "Skipping sync peer update as the current latency ({current_latency}ms) is already low enough"
            );
        }

        let our_best = self.client.best_number();

        // Find the peer with lowest latency.
        let Some(best_sync_peer) = self
            .peers
            .values()
            .filter(|peer| {
                peer.peer_id != current_sync_peer_id
                    && peer.best_number > our_best
                    && peer.state.is_available()
            })
            .min_by_key(|peer| peer.latency)
        else {
            return;
        };

        let best_latency = best_sync_peer.latency;

        // Update sync peer if the latency improvement is significant.
        if current_latency / best_latency > LATENCY_IMPROVEMENT_THRESHOLD {
            let peer_id = best_sync_peer.peer_id;
            let target_block_number =
                target_block_number(self.sync_target, best_sync_peer.best_number);

            let sync_peer_updated = match &mut self.syncing {
                Syncing::BlocksFirst(strategy) => {
                    strategy.swap_sync_peer(peer_id, target_block_number);
                    true
                }
                Syncing::HeadersFirst(strategy) => {
                    strategy.swap_sync_peer(peer_id, target_block_number);
                    true
                }
                Syncing::Idle => unreachable!("Must not be Idle as checked; qed"),
            };

            if sync_peer_updated {
                tracing::debug!(
                    old_peer_id = ?current_sync_peer_id,
                    new_peer_id = ?peer_id,
                    "🔧 Sync peer ({current_latency} ms) updated to a new peer with lower latency ({best_latency} ms)",
                );
                self.peers.entry(current_sync_peer_id).and_modify(|peer| {
                    peer.state = PeerSyncState::Available;
                });
                self.peers.entry(peer_id).and_modify(|peer| {
                    peer.state = PeerSyncState::DownloadingNew { start: our_best };
                });
            }
        }
    }

    /// Add new peer to the chain sync component and potentially starts to synchronize the network.
    pub(super) fn add_new_peer(&mut self, new_peer: NewPeer) -> SyncAction {
        let NewPeer {
            peer_id,
            best_number,
            latency,
        } = new_peer;

        let new_peer = PeerSync {
            peer_id,
            best_number,
            latency,
            state: PeerSyncState::Available,
        };

        self.peers.insert(peer_id, new_peer);

        if self.enable_block_sync {
            self.attempt_sync_start()
        } else {
            SyncAction::None
        }
    }

    pub(super) fn start_block_sync(&mut self) -> SyncAction {
        self.enable_block_sync = true;
        self.attempt_sync_start()
    }

    fn attempt_sync_start(&mut self) -> SyncAction {
        if self.syncing.is_major_syncing() {
            return SyncAction::None;
        }

        if self.peers.len() < self.min_sync_peer_threshold {
            tracing::debug!(
                "Waiting for more sync peers, discovered {} peers, require {} peers",
                self.peers.len(),
                self.min_sync_peer_threshold
            );
            return SyncAction::None;
        }

        let our_best = self.client.best_number();

        if let Some(sync_target) = self.sync_target {
            if our_best >= sync_target {
                return SyncAction::None;
            }
        }

        let find_best_available_peer = || {
            self.peers
                .iter()
                .filter(|(_peer_id, peer)| peer.best_number > our_best && peer.state.is_available())
                .min_by_key(|(_peer_id, peer)| peer.latency)
                .map(|(peer_id, _peer)| peer_id)
        };

        let find_best_deprioritized_peer = || {
            self.peers
                .iter()
                .filter(|(_peer_id, peer)| peer.best_number > our_best)
                .filter_map(|(peer_id, peer)| {
                    if let PeerSyncState::Deprioritized { stalled_count } = peer.state {
                        if stalled_count > MAX_STALLS {
                            None
                        } else {
                            Some((peer_id, stalled_count, peer.latency))
                        }
                    } else {
                        None
                    }
                })
                .min_by(
                    |(_, stalled_count_a, latency_a), (_, stalled_count_b, latency_b)| {
                        // First, compare stalled_count, then latency if stalled_count is equal
                        match stalled_count_a.cmp(stalled_count_b) {
                            std::cmp::Ordering::Equal => latency_a.cmp(latency_b), // compare latency if stalled_count is the same
                            other => other, // otherwise, return the comparison of stalled_count
                        }
                    },
                )
                .map(|(peer_id, _stalled_count, _latency)| peer_id)
        };

        let Some(next_peer_id) = find_best_available_peer()
            .or_else(find_best_deprioritized_peer)
            .copied()
        else {
            return SyncAction::None;
        };

        let Some(next_peer) = self.peers.get_mut(&next_peer_id) else {
            return SyncAction::None;
        };

        let client = self.client.clone();
        let peer_store = self.peer_store.clone();

        let peer_best = next_peer.best_number;
        let require_major_sync = peer_best - our_best > MAJOR_SYNC_GAP;

        let target_block_number = target_block_number(self.sync_target, peer_best);

        // Start major syncing if the gap is significant.
        let (new_syncing, sync_action) = if require_major_sync {
            let blocks_first = our_best >= crate::checkpoint::last_checkpoint_height()
                || matches!(self.sync_strategy, SyncStrategy::BlocksFirst);

            tracing::debug!(
                latency = ?next_peer.latency,
                "⏩ Starting major sync ({}) from {next_peer_id:?} at #{our_best}",
                if blocks_first { "blocks-first" } else { "headers-first" }
            );

            if blocks_first {
                let (sync_strategy, sync_action) =
                    BlocksFirstStrategy::new(client, next_peer_id, target_block_number, peer_store);
                (Syncing::BlocksFirst(Box::new(sync_strategy)), sync_action)
            } else {
                let (sync_strategy, sync_action) = HeadersFirstStrategy::new(
                    client,
                    self.header_verifier.clone(),
                    next_peer_id,
                    target_block_number,
                    peer_store,
                );
                (Syncing::HeadersFirst(Box::new(sync_strategy)), sync_action)
            }
        } else {
            let (sync_strategy, sync_action) =
                BlocksFirstStrategy::new(client, next_peer_id, target_block_number, peer_store);
            (Syncing::BlocksFirst(Box::new(sync_strategy)), sync_action)
        };

        next_peer.state = PeerSyncState::DownloadingNew { start: our_best };
        self.update_syncing_state(new_syncing);

        sync_action
    }

    pub(super) fn start_blocks_first_sync(&mut self) -> Option<SyncAction> {
        if matches!(self.syncing, Syncing::BlocksFirst(_)) {
            return None;
        }

        // Import the potential remaining blocks downloaded by Headers-First sync.
        self.import_pending_blocks();

        let our_best = self.client.best_number();

        let Some(best_peer) = self
            .peers
            .values()
            .filter(|peer| peer.best_number > our_best && peer.state.is_available())
            .min_by_key(|peer| peer.latency)
        else {
            self.update_syncing_state(Syncing::Idle);
            return None;
        };

        let (blocks_first_strategy, sync_action) = BlocksFirstStrategy::new(
            self.client.clone(),
            best_peer.peer_id,
            best_peer.best_number,
            self.peer_store.clone(),
        );

        tracing::debug!("Headers-First sync completed, continuing with blocks-first sync");
        self.update_syncing_state(Syncing::BlocksFirst(Box::new(blocks_first_strategy)));

        Some(sync_action)
    }

    fn update_syncing_state(&mut self, new: Syncing<Block, Client>) {
        self.syncing = new;
        self.is_major_syncing
            .store(self.syncing.is_major_syncing(), Ordering::Relaxed);
    }

    pub(super) fn set_idle(&mut self) {
        self.import_pending_blocks();

        tracing::debug!(
            best_number = self.client.best_number(),
            "Blocks-First sync completed, switching to Syncing::Idle"
        );
        self.update_syncing_state(Syncing::Idle);
    }

    // NOTE: `inv` can be received unsolicited as an announcement of a new block,
    // or in reply to `getblocks`.
    pub(super) fn on_inv(&mut self, inventories: Vec<Inventory>, from: PeerId) -> SyncAction {
        match &mut self.syncing {
            Syncing::Idle => SyncAction::None,
            Syncing::BlocksFirst(strategy) => strategy.on_inv(inventories, from),
            Syncing::HeadersFirst(_) => SyncAction::None,
        }
    }

    pub(super) fn on_block(&mut self, block: BitcoinBlock, from: PeerId) -> SyncAction {
        match &mut self.syncing {
            Syncing::Idle => SyncAction::None,
            Syncing::BlocksFirst(strategy) => strategy.on_block(block, from),
            Syncing::HeadersFirst(strategy) => strategy.on_block(block, from),
        }
    }

    pub(super) fn on_headers(&mut self, headers: Vec<BitcoinHeader>, from: PeerId) -> SyncAction {
        match &mut self.syncing {
            Syncing::HeadersFirst(strategy) => strategy.on_headers(headers, from),
            Syncing::BlocksFirst(_) | Syncing::Idle => {
                tracing::debug!(
                    ?from,
                    "Ignored headers: {:?}",
                    headers
                        .iter()
                        .map(|header| header.block_hash())
                        .collect::<Vec<_>>()
                );
                SyncAction::None
            }
        }
    }

    pub(super) fn on_blocks_processed(&mut self, results: ImportManyBlocksResult) {
        let block_downloader = match &mut self.syncing {
            Syncing::Idle => return,
            Syncing::BlocksFirst(strategy) => strategy.block_downloader(),
            Syncing::HeadersFirst(strategy) => strategy.block_downloader(),
        };
        block_downloader.handle_processed_blocks(results);
    }

    pub(super) fn import_pending_blocks(&mut self) {
        let block_downloader = match &mut self.syncing {
            Syncing::Idle => return,
            Syncing::BlocksFirst(strategy) => strategy.block_downloader(),
            Syncing::HeadersFirst(strategy) => strategy.block_downloader(),
        };

        if !block_downloader.has_pending_blocks() {
            return;
        }

        let (hashes, blocks) = block_downloader.prepare_blocks_for_import(self.sync_target);

        tracing::trace!(
            blocks = ?hashes,
            blocks_in_queue = block_downloader.blocks_in_queue_count(),
            "Scheduling {} blocks for import",
            blocks.len(),
        );

        self.import_queue.import_blocks(ImportBlocks {
            origin: if self.is_major_syncing.load(Ordering::Relaxed) {
                BlockOrigin::NetworkInitialSync
            } else {
                BlockOrigin::NetworkBroadcast
            },
            blocks,
        });
    }
}

/// Determines the target block number for syncing based on the provided sync target
/// and the peer's best block number.
///
/// Bitcoin Core only supports snapshots at specific block heights (e.g., 840000 as of writing).
/// To avoid syncing past a block that may have been pruned or is unavailable in pruning mode
/// when running a snapshot node, this function ensures that we do not sync beyond a certain
/// block height, as determined by the `sync_target` or the peer's best block height.
fn target_block_number(sync_target: Option<u32>, peer_best: u32) -> u32 {
    match sync_target {
        Some(target) => peer_best.min(target),
        None => peer_best,
    }
}
