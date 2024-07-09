use crate::block_downloader::BlockDownloadManager;
use crate::sync::{LocatorRequest, SyncAction, SyncRequest};
use crate::{Error, PeerId, SyncStatus};
use bitcoin::blockdata::block::Header as BitcoinHeader;
use bitcoin::p2p::message_blockdata::Inventory;
use bitcoin::{Block as BitcoinBlock, BlockHash};
use indexmap::IndexMap;
use sc_client_api::AuxStore;
use sp_blockchain::HeaderBackend;
use sp_runtime::traits::Block as BlockT;
use std::collections::{HashSet, VecDeque};
use std::fmt::Display;
use std::marker::PhantomData;
use std::net::IpAddr;
use std::sync::Arc;
use subcoin_primitives::{BackendExt, BlockLocatorProvider, ClientExt, IndexedBlock};

/// Represents the range of blocks to be downloaded during the headers-first sync.
///
/// # Note
///
/// The `getdata` request for blocks must be ordered before sending them to the peer,
/// this does not guarantee the block responses from peers are ordered, but it's helpful
/// for the peers with good connections (i.e., when syncing from a local node).
#[derive(Debug, Clone)]
enum BlockDownloadRange {
    /// Request all blocks within the specified range in a single `getdata` message.
    ///
    /// This option is only used when the sync node is a local node.
    AllBlocks {
        start: IndexedBlock,
        end: IndexedBlock,
    },
    /// Request blocks in smaller, manageable batches.
    Batches {
        /// Tracks the number of batches already downloaded.
        downloaded_batch: usize,
        /// A queue of sets containing the blocks that are waiting to be downloaded.
        ///
        /// **Note**: The blocks currently being downloaded are tracked by the sync
        /// manager, not included directly in this structure..
        waiting: VecDeque<HashSet<BlockHash>>,
        /// Is the blocks download paused?
        paused: bool,
    },
}

#[derive(Debug, Clone)]
enum DownloadState {
    /// Downloading not started yet.
    Idle,
    /// Restarting the download process.
    Restarting,
    /// Peer misbehavior detected, will disconnect the peer shortly.
    Disconnecting,
    /// Actively downloading new headers in the specified range (start, end].
    ///
    /// Block at the height `start` already exists in our system, `start` being
    /// exclusive is to quickly verify the parent block of first header in the response.
    DownloadingHeaders {
        start: IndexedBlock,
        end: IndexedBlock,
    },
    /// Headers downloaded, Actively downloading the corresponding block data (start, end].
    DownloadingBlocks(BlockDownloadRange),
    /// All blocks up to the target block have been successfully
    /// downloaded, the download process has been completed.
    Completed,
}

impl Display for DownloadState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Idle => write!(f, "Idle"),
            Self::Restarting => write!(f, "Restarting"),
            Self::Disconnecting => write!(f, "Disconnecting"),
            Self::DownloadingHeaders { start, end } => {
                write!(f, "DownloadingHeaders {{ start: {}, end: {} }}", start, end)
            }
            Self::DownloadingBlocks(_range) => {
                write!(f, "DownloadingBlocks")
            }
            Self::Completed => write!(f, "Completed"),
        }
    }
}

fn block_download_batch_size(height: u32) -> usize {
    match height {
        0..=99_999 => 1024,
        100_000..=199_999 => 512,
        200_000..=299_999 => 128,
        300_000..=399_999 => 64,
        400_000..=499_999 => 32,
        _ => 16,
    }
}

/// Headers-First download strategy.
pub struct HeadersFirstDownloader<Block, Client> {
    client: Arc<Client>,
    peer_id: PeerId,
    download_state: DownloadState,
    download_manager: BlockDownloadManager,
    // Keep the headers ordered so that fetching the blocks orderly later is possible.
    downloaded_headers: IndexMap<BlockHash, u32>,
    last_locator_start: u32,
    // TODO: Now it's solely used for the purpose of displaying the sync state.
    // refactor it later.
    target_block_number: u32,
    _phantom: PhantomData<Block>,
}

impl<Block, Client> HeadersFirstDownloader<Block, Client>
where
    Block: BlockT,
    Client: HeaderBackend<Block> + AuxStore,
{
    pub(crate) fn new(
        client: Arc<Client>,
        peer_id: PeerId,
        target_block_number: u32,
    ) -> (Self, SyncAction) {
        let mut headers_first_sync = Self {
            client,
            peer_id,
            download_state: DownloadState::Idle,
            downloaded_headers: IndexMap::new(),
            download_manager: BlockDownloadManager::new(),
            last_locator_start: 0u32,
            target_block_number,
            _phantom: Default::default(),
        };
        let sync_action = headers_first_sync.prepare_headers_request_action();
        (headers_first_sync, sync_action)
    }

    pub(crate) fn sync_status(&self) -> SyncStatus {
        if self.download_manager.import_queue_is_overloaded {
            SyncStatus::Importing {
                target: self.target_block_number,
                peers: vec![self.peer_id],
            }
        } else {
            SyncStatus::Downloading {
                target: self.target_block_number,
                peers: vec![self.peer_id],
            }
        }
    }

    pub(crate) fn sync_peer(&self) -> PeerId {
        self.peer_id
    }

    pub(crate) fn update_sync_peer(&mut self, peer_id: PeerId, target_block_number: u32) {
        self.peer_id = peer_id;
        self.target_block_number = target_block_number;
    }

    pub(crate) fn on_tick(&mut self) -> SyncAction {
        if matches!(self.download_state, DownloadState::Restarting) {
            return self.prepare_headers_request_action();
        }

        if self.download_manager.import_queue_is_overloaded {
            let import_queue_still_busy = self
                .download_manager
                .update_and_check_queue_status(self.client.best_number());
            if import_queue_still_busy {
                return SyncAction::None;
            } else {
                // Resume blocks or headers request.
                match &mut self.download_state {
                    DownloadState::DownloadingBlocks(BlockDownloadRange::Batches {
                        downloaded_batch,
                        waiting,
                        paused,
                    }) if *paused => {
                        *paused = false;

                        if let Some(next_batch) = waiting.pop_front() {
                            tracing::debug!(
                                best_number = self.client.best_number(),
                                best_queued_number = self.download_manager.best_queued_number,
                                "📦 Resumed downloading {} blocks in batches ({}/{})",
                                next_batch.len(),
                                *downloaded_batch + 1,
                                *downloaded_batch + 1 + waiting.len()
                            );
                            self.download_manager
                                .requested_blocks
                                .clone_from(&next_batch);
                            return self.blocks_request_action(next_batch);
                        } else {
                            return self.prepare_headers_request_action();
                        }
                    }
                    _ => {
                        return self.prepare_headers_request_action();
                    }
                }
            }
        }

        if self.download_manager.is_stalled() {
            return SyncAction::RestartSyncWithStalledPeer(self.peer_id);
        }

        SyncAction::None
    }

    pub(crate) fn restart(&mut self, new_peer: PeerId, peer_best: u32) {
        self.peer_id = new_peer;
        self.downloaded_headers.clear();
        self.download_manager.reset();
        self.last_locator_start = 0u32;
        self.target_block_number = peer_best;
        self.download_state = DownloadState::Restarting;
    }

    fn prepare_headers_request_action(&mut self) -> SyncAction {
        let our_best = self.client.best_number();

        let Some(checkpoint) = crate::checkpoint::next_checkpoint(our_best + 1) else {
            tracing::debug!(
                our_best,
                "No more checkpoints, switching to blocks-first sync"
            );
            return SyncAction::SwitchToBlocksFirstSync;
        };

        let start = IndexedBlock {
            number: our_best,
            hash: self
                .client
                .block_hash(our_best)
                .expect("Best block must exist; qed"),
        };

        let end = checkpoint;

        tracing::debug!("Requesting headers from {start} to {end}");

        // Ignore the back-to-back duplicate locators.
        if our_best > 0 && our_best == self.last_locator_start {
            return SyncAction::None;
        }

        self.last_locator_start = our_best;

        let locator_hashes = self
            .client
            .block_locator(Some(our_best), |_height: u32| None)
            .locator_hashes;

        self.download_state = DownloadState::DownloadingHeaders { start, end };

        SyncAction::Request(SyncRequest::Headers(LocatorRequest {
            locator_hashes,
            stop_hash: checkpoint.hash,
            from: self.peer_id,
        }))
    }

    pub(crate) fn download_manager(&mut self) -> &mut BlockDownloadManager {
        &mut self.download_manager
    }

    // Handle `headers` message.
    //
    // `headers` are expected to contain at most 2000 entries, in ascending order. [b1, b2, b3, ..., b2000].
    pub(crate) fn on_headers(&mut self, headers: Vec<BitcoinHeader>, from: PeerId) -> SyncAction {
        // https://developer.bitcoin.org/reference/p2p_networking.html#headers
        const MAX_HEADERS_SIZE: usize = 2000;

        if headers.len() > MAX_HEADERS_SIZE {
            return SyncAction::Disconnect(from, Error::TooManyHeaders);
        }

        let Some(first_header) = headers.first() else {
            tracing::debug!("Received empty response of getheaders");
            return SyncAction::None;
        };

        let (start, end) = match &self.download_state {
            DownloadState::DownloadingHeaders { start, end } => (*start, *end),
            state => {
                tracing::debug!(
                    %state,
                    "Ignoring headers as we are not in the mode of downloading headers"
                );
                return SyncAction::None;
            }
        };

        let mut prev_hash = first_header.prev_blockhash;

        let mut prev_number = if prev_hash == start.hash {
            start.number
        } else if let Some(block_number) = self.downloaded_headers.get(&prev_hash).copied() {
            block_number
        } else if let Some(block_number) = self.client.block_number(prev_hash) {
            block_number
        } else {
            tracing::info!(
                ?first_header,
                best_number = ?self.client.info().best_number,
                "Cannot find the parent of the first header in headers, disconnecting"
            );
            self.download_state = DownloadState::Disconnecting;
            return SyncAction::Disconnect(
                self.peer_id,
                Error::Other("Cannot find the parent of the first header".to_string()),
            );
        };

        for header in headers {
            if header.prev_blockhash != prev_hash {
                self.download_state = DownloadState::Disconnecting;
                return SyncAction::Disconnect(
                    self.peer_id,
                    Error::Other("Invalid headers: not in ascending order".to_string()),
                );
            }

            // TODO: Verify header?

            let block_hash = header.block_hash();
            let block_number = prev_number + 1;

            // We can't import the header directly at this moment since creating a Substrate
            // header requires the full block data.
            self.downloaded_headers.insert(block_hash, block_number);

            prev_hash = block_hash;
            prev_number = block_number;
        }

        let final_block_number = prev_number;
        let target_block_number = end.number;
        let target_block_hash = end.hash;

        if final_block_number == target_block_number {
            self.start_block_download(start, end)
        } else {
            tracing::debug!("📄 Downloading headers ({final_block_number}/{target_block_number})");

            SyncAction::Request(SyncRequest::Headers(LocatorRequest {
                locator_hashes: vec![prev_hash],
                stop_hash: target_block_hash,
                from: self.peer_id,
            }))
        }
    }

    // Fetch the block data of headers we have just downloaded.
    fn start_block_download(&mut self, start: IndexedBlock, end: IndexedBlock) -> SyncAction {
        // TODO: sync blocks from multiple peers in parallel.

        let best_number = self.client.best_number();

        let downloaded_headers = self.downloaded_headers.len();
        let missing_blocks =
            self.downloaded_headers
                .iter()
                .filter_map(|(block_hash, block_number)| {
                    let block_hash = *block_hash;

                    if *block_number > best_number {
                        return Some(block_hash);
                    }

                    if self.client.block_exists(block_hash) {
                        None
                    } else {
                        Some(block_hash)
                    }
                });

        // If the sync peer is running from local, the bandwidth is not a bottleneck,
        // simply request all blocks at once.
        let download_all_blocks_in_one_request = is_local_address(&self.peer_id);

        let get_data_msg = if download_all_blocks_in_one_request {
            let get_data_msg = missing_blocks.map(Inventory::Block).collect::<Vec<_>>();

            tracing::debug!(
                best_number,
                best_queued_number = self.download_manager.best_queued_number,
                requested_blocks_count = get_data_msg.len(),
                downloaded_headers,
                "Headers from {start} to {end} downloaded, requesting blocks",
            );

            self.download_state =
                DownloadState::DownloadingBlocks(BlockDownloadRange::AllBlocks { start, end });

            get_data_msg
        } else {
            let batch_size = block_download_batch_size(end.number);
            let mut batches = missing_blocks
                .collect::<Vec<_>>()
                .chunks(batch_size)
                .map(|set| HashSet::from_iter(set.to_vec()))
                .collect::<VecDeque<HashSet<_>>>();

            let total_batches = batches.len();
            let initial_batch = batches.pop_front().expect("Batch must not be empty; qed");

            let get_data_msg =
                prepare_ordered_block_data_request(initial_batch.clone(), &self.downloaded_headers);

            let old_requested =
                std::mem::replace(&mut self.download_manager.requested_blocks, initial_batch);

            assert!(
                old_requested.is_empty(),
                "There are still requested blocks not yet received: {old_requested:?}"
            );

            tracing::debug!(
                best_number,
                best_queued_number = self.download_manager.best_queued_number,
                downloaded_headers,
                "Headers downloaded, requesting {} blocks in batches (1/{total_batches})",
                get_data_msg.len(),
            );

            self.download_state = DownloadState::DownloadingBlocks(BlockDownloadRange::Batches {
                downloaded_batch: 0,
                waiting: batches,
                paused: false,
            });

            get_data_msg
        };

        SyncAction::Request(SyncRequest::Data(get_data_msg, self.peer_id))
    }

    pub(crate) fn on_block(&mut self, block: BitcoinBlock, from: PeerId) -> SyncAction {
        let block_download_range = match &mut self.download_state {
            DownloadState::DownloadingBlocks(download_range) => download_range,
            state => {
                tracing::warn!(
                    ?state,
                    ?from,
                    current_sync_peer = ?self.peer_id,
                    "Not in the block download mode, dropping block {}",
                    block.block_hash()
                );
                return SyncAction::None;
            }
        };

        let block_hash = block.block_hash();

        let receive_requested_block = self.download_manager.on_block_response(block_hash);

        let parent_block_hash = block.header.prev_blockhash;

        let maybe_parent =
            if let Some(number) = self.download_manager.block_number(parent_block_hash) {
                Some(number)
            } else {
                self.client.block_number(parent_block_hash)
            };

        if let Some(parent_block_number) = maybe_parent {
            let block_number = parent_block_number + 1;

            tracing::trace!("Add pending block #{block_number},{block_hash}");

            self.download_manager
                .add_block(block_number, block_hash, block);

            let should_request_more_headers = match block_download_range {
                BlockDownloadRange::AllBlocks { start, end } => {
                    if end.hash == block_hash {
                        tracing::debug!("Downloaded blocks in ({start}, {end}]");
                        true
                    } else {
                        false
                    }
                }
                BlockDownloadRange::Batches {
                    downloaded_batch,
                    waiting,
                    paused,
                } => {
                    if self.download_manager.requested_blocks.is_empty() {
                        *downloaded_batch += 1;

                        if self
                            .download_manager
                            .update_and_check_queue_status(self.client.best_number())
                        {
                            *paused = true;
                            return SyncAction::None;
                        }

                        if let Some(next_batch) = waiting.pop_front() {
                            tracing::debug!(
                                best_number = self.client.best_number(),
                                best_queued_number = self.download_manager.best_queued_number,
                                "📦 Downloading {} blocks in batches ({}/{})",
                                next_batch.len(),
                                *downloaded_batch + 1,
                                *downloaded_batch + 1 + waiting.len()
                            );
                            self.download_manager
                                .requested_blocks
                                .clone_from(&next_batch);
                            return self.blocks_request_action(next_batch);
                        }

                        tracing::debug!("Downloaded checkpoint block #{block_number},{block_hash}");

                        true
                    } else {
                        false
                    }
                }
            };

            if should_request_more_headers {
                self.request_more_headers_at_checkpoint(block_number, block_hash)
            } else {
                SyncAction::None
            }
        } else {
            if receive_requested_block {
                self.download_manager.add_orphan_block(block_hash, block);
            } else {
                tracing::debug!("Discard unrequested orphan block {block_hash}");
            }

            SyncAction::None
        }
    }

    fn blocks_request_action(&self, blocks_to_download: HashSet<BlockHash>) -> SyncAction {
        let get_data_msg =
            prepare_ordered_block_data_request(blocks_to_download, &self.downloaded_headers);
        SyncAction::Request(SyncRequest::Data(get_data_msg, self.peer_id))
    }

    // All blocks for the downloaded headers have been downloaded, start to request
    // more headers if the next checkpoint exists.
    fn request_more_headers_at_checkpoint(
        &mut self,
        block_number: u32,
        block_hash: BlockHash,
    ) -> SyncAction {
        let best_number = self.client.best_number();

        if self
            .download_manager
            .update_and_check_queue_status(best_number)
        {
            return SyncAction::None;
        }

        match crate::checkpoint::next_checkpoint(block_number + 1) {
            Some(checkpoint) => {
                self.download_state = DownloadState::DownloadingHeaders {
                    start: IndexedBlock {
                        number: block_number,
                        hash: block_hash,
                    },
                    end: checkpoint,
                };

                tracing::debug!(
                    "Fetching {} headers up to the next checkpoint {checkpoint}",
                    checkpoint.number - block_number,
                );

                SyncAction::Request(SyncRequest::Headers(LocatorRequest {
                    locator_hashes: vec![block_hash],
                    stop_hash: checkpoint.hash,
                    from: self.peer_id,
                }))
            }
            None => {
                // We have synced to the last checkpoint, now switch to the Blocks-First
                // sync to download the remaining blocks.
                tracing::debug!("No more checkpoint, switching to blocks-first sync");
                self.download_state = DownloadState::Completed;
                SyncAction::SwitchToBlocksFirstSync
            }
        }
    }
}

fn prepare_ordered_block_data_request(
    blocks: HashSet<BlockHash>,
    downloaded_headers: &IndexMap<BlockHash, u32>,
) -> Vec<Inventory> {
    let mut blocks = blocks
        .into_iter()
        .map(|block_hash| {
            (
                downloaded_headers.get(&block_hash).expect(
                    "Header of the block to download must exist in headers-first sync; qed",
                ),
                block_hash,
            )
        })
        .collect::<Vec<_>>();

    blocks.sort_by(|a, b| a.0.cmp(b.0));

    blocks
        .into_iter()
        .map(|(_number, hash)| Inventory::Block(hash))
        .collect()
}

fn is_local_address(addr: &PeerId) -> bool {
    match addr.ip() {
        IpAddr::V4(ipv4) => ipv4.is_loopback() || ipv4.is_private() || ipv4.is_unspecified(),
        IpAddr::V6(_ipv6) => false,
    }
}
