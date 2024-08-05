use crate::block_downloader::{BlocksFirstDownloader, HeadersFirstDownloader};
use crate::peer_manager::NewPeer;
use crate::{Error, Latency, PeerId, SyncStatus, SyncStrategy};
use bitcoin::blockdata::block::Header as BitcoinHeader;
use bitcoin::p2p::message_blockdata::Inventory;
use bitcoin::{Block as BitcoinBlock, BlockHash};
use futures::StreamExt;
use sc_client_api::{AuxStore, HeaderBackend};
use sc_consensus_nakamoto::{BlockImportQueue, ImportBlocks, ImportManyBlocksResult};
use serde::{Deserialize, Serialize};
use sp_consensus::BlockOrigin;
use sp_runtime::traits::Block as BlockT;
use std::cmp::Ordering as CmpOrdering;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use subcoin_primitives::ClientExt;

// Do major sync when the current tip falls behind the network by 144 blocks (roughly one day).
const MAJOR_SYNC_GAP: u32 = 144;

const LATENCY_IMPROVEMENT_THRESHOLD: f64 = 1.2;

/// The state of syncing between a Peer and ourselves.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PeerSyncState {
    /// Available for sync requests.
    Available,
    /// The peer has been discouraged due to syncing from this peer was stalled before.
    Discouraged,
    /// Actively downloading new blocks, starting from the given Number.
    DownloadingNew { start: u32 },
}

impl PeerSyncState {
    /// Returns `true` if the peer is available for syncing.
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available)
    }
}

/// Ping letency of the peer.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PeerLatency {
    Unknown,
    Average(Latency),
}

impl Ord for PeerLatency {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        match (self, other) {
            (Self::Unknown, Self::Unknown) => CmpOrdering::Equal,
            (Self::Unknown, Self::Average(_)) => CmpOrdering::Less,
            (Self::Average(_), Self::Unknown) => CmpOrdering::Greater,
            (Self::Average(a), Self::Average(b)) => a.cmp(b),
        }
    }
}

impl PartialOrd for PeerLatency {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
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
    pub latency: PeerLatency,
    /// The state of syncing this peer is in for us, generally categories
    /// into `Available` or "busy" with something as defined by `PeerSyncState`.
    pub state: PeerSyncState,
}

/// Locator based sync request, for requesting either Headers or Blocks.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct LocatorRequest {
    pub locator_hashes: Vec<BlockHash>,
    pub stop_hash: BlockHash,
    pub from: PeerId,
}

/// Represents different kinds of sync requests.
#[derive(Debug)]
pub(crate) enum SyncRequest {
    /// Get headers request.
    Headers(LocatorRequest),
    /// Get blocks request.
    Blocks(LocatorRequest),
    /// Get data request.
    Data(Vec<Inventory>, PeerId),
}

/// Represents actions that can be taken during the syncing.
#[derive(Debug)]
pub(crate) enum SyncAction {
    /// Fetch headers, blocks and data.
    Request(SyncRequest),
    /// Headers-First sync completed, use the Blocks-First sync
    /// to download the recent blocks.
    SwitchToBlocksFirstSync,
    /// Disconnect from the peer for the given reason.
    Disconnect(PeerId, Error),
    /// Make this peer as discouraged and restart the current syncing
    /// process using other sync candidates if there are any.
    RestartSyncWithStalledPeer(PeerId),
    /// No action needed.
    None,
}

// This enum encapsulates the various strategies and states a node
// might be in during the sync process.
enum Syncing<Block, Client> {
    /// Blocks-First sync.
    BlocksFirstSync(BlocksFirstDownloader<Block, Client>),
    /// Headers-First sync.
    HeadersFirstSync(HeadersFirstDownloader<Block, Client>),
    /// Not syncing.
    ///
    /// This could indicate that the node is either fully synced
    /// or is waiting for more peers to resume syncing.
    Idle,
}

impl<Block, Client> Syncing<Block, Client> {
    fn is_major_syncing(&self) -> bool {
        matches!(self, Self::BlocksFirstSync(_) | Self::HeadersFirstSync(_))
    }
}

/// The main data structure which contains all the state for syncing.
pub(crate) struct ChainSync<Block, Client> {
    /// Chain client.
    client: Arc<Client>,
    /// The active peers that we are using to sync and their PeerSync status
    pub(crate) peers: HashMap<PeerId, PeerSync>,
    syncing: Syncing<Block, Client>,
    /// Handle of the import queue.
    import_queue: BlockImportQueue,
    sync_strategy: SyncStrategy,
    is_major_syncing: Arc<AtomicBool>,
    rng: fastrand::Rng,
    _phantom: PhantomData<Block>,
}

impl<Block, Client> ChainSync<Block, Client>
where
    Block: BlockT,
    Client: HeaderBackend<Block> + AuxStore,
{
    /// Constructs a new instance of [`ChainSync`].
    pub(super) fn new(
        client: Arc<Client>,
        import_queue: BlockImportQueue,
        sync_strategy: SyncStrategy,
        is_major_syncing: Arc<AtomicBool>,
    ) -> Self {
        Self {
            client,
            peers: HashMap::new(),
            import_queue,
            syncing: Syncing::Idle,
            sync_strategy,
            is_major_syncing,
            rng: fastrand::Rng::new(),
            _phantom: Default::default(),
        }
    }

    pub(super) fn sync_status(&self) -> SyncStatus {
        match &self.syncing {
            Syncing::Idle => SyncStatus::Idle,
            Syncing::BlocksFirstSync(downloader) => downloader.sync_status(),
            Syncing::HeadersFirstSync(downloader) => downloader.sync_status(),
        }
    }

    pub(super) fn on_tick(&mut self) -> SyncAction {
        match &mut self.syncing {
            Syncing::Idle => SyncAction::None,
            Syncing::BlocksFirstSync(downloader) => downloader.on_tick(),
            Syncing::HeadersFirstSync(downloader) => downloader.on_tick(),
        }
    }

    pub(super) async fn wait_for_block_import_results(&mut self) -> ImportManyBlocksResult {
        self.import_queue
            .import_result_receiver
            .select_next_some()
            .await
    }

    /// Attempts to restart the sync due to the stalled peer.
    ///
    /// Returns `true` if the sync is restarted with a new peer.
    pub(super) fn restart_sync(&mut self, stalled_peer: PeerId) -> bool {
        let our_best = self.client.best_number();

        // First, try to find the best available peer for syncing.
        let new_available_peer = self
            .peers
            .values_mut()
            .filter(|peer| {
                peer.peer_id != stalled_peer
                    && peer.best_number > our_best
                    && peer.state.is_available()
            })
            .max_by_key(|peer| peer.best_number);

        let new_peer = match new_available_peer {
            Some(peer) => peer,
            None => {
                let sync_candidates = self
                    .peers
                    .values_mut()
                    .filter(|peer| peer.peer_id != stalled_peer && peer.best_number > our_best)
                    .collect::<Vec<_>>();

                // No new sync candidate, keep it as is.
                if sync_candidates.is_empty() {
                    tracing::debug!(?stalled_peer, "⚠️ Sync stalled, but no new sync candidates");
                    return false;
                }

                // Pick a random peer, even if it's marked as discouraged.
                self.rng
                    .choice(sync_candidates)
                    .expect("Sync candidates must be non-empty as checked; qed")
            }
        };

        tracing::debug!(?stalled_peer, ?new_peer, "🔄 Sync stalled, restarting");

        new_peer.state = PeerSyncState::DownloadingNew { start: our_best };

        match &mut self.syncing {
            Syncing::BlocksFirstSync(downloader) => {
                downloader.restart(new_peer.peer_id, new_peer.best_number);
                true
            }
            Syncing::HeadersFirstSync(downloader) => {
                downloader.restart(new_peer.peer_id, new_peer.best_number);
                true
            }
            Syncing::Idle => false,
        }
    }

    pub(super) fn mark_peer_as_discouraged(&mut self, stalled_peer: PeerId) {
        self.peers
            .entry(stalled_peer)
            .and_modify(|p| p.state = PeerSyncState::Discouraged);
    }

    pub(super) fn remove_peer(&mut self, peer_id: PeerId) {
        // TODO: handle the situation that the peer is being involved in the downloader.
        self.peers.remove(&peer_id);
    }

    pub(super) fn set_peer_latency(&mut self, peer_id: PeerId, latency: Latency) {
        self.peers.entry(peer_id).and_modify(|peer| {
            peer.latency = PeerLatency::Average(latency);
        });
    }

    pub(super) fn update_sync_peer_on_lower_latency(&mut self) {
        let current_sync_peer_id = match &self.syncing {
            Syncing::BlocksFirstSync(downloader) => downloader.sync_peer(),
            Syncing::HeadersFirstSync(downloader) => downloader.sync_peer(),
            Syncing::Idle => return,
        };

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

        if let Some(current_sync_peer) = self.peers.get(&current_sync_peer_id) {
            if let (PeerLatency::Average(current_latency), PeerLatency::Average(best_latency)) =
                (current_sync_peer.latency, best_sync_peer.latency)
            {
                // Update sync peer if the latency improvement is significant.
                if current_latency as f64 / best_latency as f64 > LATENCY_IMPROVEMENT_THRESHOLD {
                    let peer_id = best_sync_peer.peer_id;
                    let target_block_number = best_sync_peer.best_number;

                    match &mut self.syncing {
                        Syncing::BlocksFirstSync(downloader) => {
                            downloader.update_sync_peer(peer_id, target_block_number);
                        }
                        Syncing::HeadersFirstSync(downloader) => {
                            downloader.update_sync_peer(peer_id, target_block_number);
                        }
                        Syncing::Idle => unreachable!("Must not be Idle as checked; qed"),
                    }

                    tracing::debug!(
                        old_peer_id = ?current_sync_peer_id,
                        new_peer_id = ?peer_id,
                        "🔧 Sync peer ({current_latency} ms) updated to a new peer with lower latency ({best_latency} ms)",
                    );
                }
            }
        }
    }

    /// Add new peer to the chain sync component and potentially starts to synchronize the network.
    pub(super) fn add_new_peer(&mut self, new_peer: NewPeer) -> SyncAction {
        let NewPeer {
            peer_id,
            best_number,
        } = new_peer;

        let new_peer = PeerSync {
            peer_id,
            best_number,
            latency: PeerLatency::Unknown,
            state: PeerSyncState::Available,
        };

        self.peers.insert(peer_id, new_peer);

        self.attempt_sync_start()
    }

    fn attempt_sync_start(&mut self) -> SyncAction {
        if self.syncing.is_major_syncing() {
            return SyncAction::None;
        }

        let our_best = self.client.best_number();

        let Some(best_peer) = self
            .peers
            .values_mut()
            .filter(|peer| peer.best_number > our_best && peer.state.is_available())
            .min_by_key(|peer| peer.latency)
        else {
            return SyncAction::None;
        };

        let peer_best = best_peer.best_number;

        if peer_best > our_best {
            let sync_peer = best_peer.peer_id;

            best_peer.state = PeerSyncState::DownloadingNew { start: our_best };

            let require_major_sync = peer_best - our_best > MAJOR_SYNC_GAP;

            // Start major syncing if the gap is significant.
            let (new_syncing, sync_action) = if require_major_sync {
                tracing::debug!(
                    from = ?sync_peer,
                    start = our_best,
                    latency = ?best_peer.latency,
                    "⏩ Starting major sync",
                );

                match self.sync_strategy {
                    SyncStrategy::BlocksFirst => {
                        let (blocks_first_downloader, blocks_sync_request) =
                            BlocksFirstDownloader::new(self.client.clone(), sync_peer, peer_best);
                        (
                            Syncing::BlocksFirstSync(blocks_first_downloader),
                            SyncAction::Request(blocks_sync_request),
                        )
                    }
                    SyncStrategy::HeadersFirst => {
                        let (headers_first_downloader, sync_action) =
                            HeadersFirstDownloader::new(self.client.clone(), sync_peer, peer_best);
                        (
                            Syncing::HeadersFirstSync(headers_first_downloader),
                            sync_action,
                        )
                    }
                }
            } else {
                let (blocks_first_downloader, blocks_sync_request) =
                    BlocksFirstDownloader::new(self.client.clone(), sync_peer, peer_best);
                (
                    Syncing::BlocksFirstSync(blocks_first_downloader),
                    SyncAction::Request(blocks_sync_request),
                )
            };

            self.update_syncing_state(new_syncing);

            return sync_action;
        }

        SyncAction::None
    }

    fn update_syncing_state(&mut self, new: Syncing<Block, Client>) {
        let is_major_syncing = new.is_major_syncing();
        self.syncing = new;
        self.is_major_syncing
            .store(is_major_syncing, Ordering::Relaxed);
    }

    pub(super) fn attempt_blocks_first_sync(&mut self) -> Option<SyncRequest> {
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

        let (blocks_first_downloader, blocks_sync_request) = BlocksFirstDownloader::new(
            self.client.clone(),
            best_peer.peer_id,
            best_peer.best_number,
        );

        self.update_syncing_state(Syncing::BlocksFirstSync(blocks_first_downloader));

        Some(blocks_sync_request)
    }

    // NOTE: `inv` can be received unsolicited as an announcement of a new block,
    // or in reply to `getblocks`.
    pub(super) fn on_inv(&mut self, inventories: Vec<Inventory>, from: PeerId) -> SyncAction {
        match &mut self.syncing {
            Syncing::BlocksFirstSync(downloader) => downloader.on_inv(inventories, from),
            Syncing::HeadersFirstSync(_) => SyncAction::None,
            Syncing::Idle => {
                // TODO: A new block maybe broadcasted via `inv` message.
                SyncAction::None
            }
        }
    }

    pub(super) fn on_block(&mut self, block: BitcoinBlock, from: PeerId) -> SyncAction {
        match &mut self.syncing {
            Syncing::Idle => SyncAction::None,
            Syncing::BlocksFirstSync(downloader) => downloader.on_block(block, from),
            Syncing::HeadersFirstSync(downloader) => downloader.on_block(block, from),
        }
    }

    pub(super) fn on_headers(&mut self, headers: Vec<BitcoinHeader>, from: PeerId) -> SyncAction {
        match &mut self.syncing {
            Syncing::HeadersFirstSync(downloader) => downloader.on_headers(headers, from),
            Syncing::BlocksFirstSync(_) => SyncAction::None,
            Syncing::Idle => {
                // TODO: A new block maybe broadcasted via `headers` message.
                SyncAction::None
            }
        }
    }

    pub(super) fn on_blocks_processed(&mut self, results: ImportManyBlocksResult) {
        let download_manager = match &mut self.syncing {
            Syncing::Idle => return,
            Syncing::BlocksFirstSync(downloader) => downloader.download_manager(),
            Syncing::HeadersFirstSync(downloader) => downloader.download_manager(),
        };
        download_manager.handle_processed_blocks(results);
    }

    pub(super) fn import_pending_blocks(&mut self) {
        let download_manager = match &mut self.syncing {
            Syncing::Idle => return,
            Syncing::BlocksFirstSync(downloader) => downloader.download_manager(),
            Syncing::HeadersFirstSync(downloader) => downloader.download_manager(),
        };

        if !download_manager.has_pending_blocks() {
            return;
        }

        let (hashes, blocks) = download_manager.prepare_blocks_for_import();

        tracing::trace!(
            blocks = ?hashes,
            blocks_in_queue = download_manager.blocks_in_queue_count(),
            "Scheduling {} blocks for import",
            blocks.len(),
        );

        self.import_queue.import_blocks(ImportBlocks {
            origin: BlockOrigin::NetworkInitialSync,
            blocks,
        });
    }
}
