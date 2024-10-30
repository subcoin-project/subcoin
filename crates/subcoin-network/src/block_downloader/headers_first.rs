use crate::block_downloader::BlockDownloader;
use crate::peer_store::PeerStore;
use crate::sync::{LocatorRequest, SyncAction, SyncRequest};
use crate::{Error, PeerId, SyncStatus};
use bitcoin::blockdata::block::Header as BitcoinHeader;
use bitcoin::p2p::message_blockdata::Inventory;
use bitcoin::{Block as BitcoinBlock, BlockHash};
use indexmap::IndexMap;
use sc_client_api::AuxStore;
use sc_consensus_nakamoto::HeaderVerifier;
use sp_blockchain::HeaderBackend;
use sp_runtime::traits::Block as BlockT;
use std::collections::{HashSet, VecDeque};
use std::fmt::Display;
use std::marker::PhantomData;
use std::net::IpAddr;
use std::sync::Arc;
use subcoin_primitives::{BackendExt, BlockLocatorProvider, ClientExt, IndexedBlock};

// https://developer.bitcoin.org/reference/p2p_networking.html#headers
const MAX_HEADERS_SIZE: usize = 2000;

/// Represents the range of blocks to be downloaded during the headers-first sync.
///
/// # Note
///
/// The `getdata` request for blocks must be ordered before sending them to the peer,
/// this does not guarantee the block responses from peers are ordered, but it's helpful
/// for the peers with good connections (i.e., when syncing from a local node).
#[derive(Debug, Clone)]
enum BlockDownload {
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

/// Represents the current state of the download process.
#[derive(Debug, Clone)]
enum State {
    /// Downloading not started yet.
    Idle,
    /// Restarting the download by requesting headers.
    RestartingHeaders,
    /// Restarting the download by continuing with block download from the specified range.
    RestartingBlocks {
        start: IndexedBlock,
        end: IndexedBlock,
    },
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
    /// Actively downloading blocks corresponding to previously downloaded headers (start, end].
    DownloadingBlocks(BlockDownload),
    /// All blocks up to the target block have been successfully
    /// downloaded, the download process has been completed.
    Completed,
}

impl Display for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Idle => write!(f, "Idle"),
            Self::RestartingHeaders => write!(f, "RestartingHeaders"),
            Self::RestartingBlocks { start, end } => {
                write!(f, "RestartingBlocks {{ start: {start}, end: {end} }}")
            }
            Self::Disconnecting => write!(f, "Disconnecting"),
            Self::DownloadingHeaders { start, end } => {
                write!(f, "DownloadingHeaders {{ start: {start}, end: {end} }}")
            }
            Self::DownloadingBlocks(_range) => write!(f, "DownloadingBlocks"),
            Self::Completed => write!(f, "Completed"),
        }
    }
}

fn block_download_batch_size(height: u32) -> usize {
    match height {
        0..=99_999 => 1024,
        100_000..=199_999 => 512,
        200_000..=299_999 => 64,
        300_000..=499_999 => 16,
        _ => 8,
    }
}

/// Headers downloaded up to the next checkpoint.
#[derive(Default)]
struct DownloadedHeaders {
    /// Ordered map of headers, where the key is the block hash and the value is the block number.
    ///
    /// Keep the headers ordered so that fetching the blocks orderly later is possible.
    headers: IndexMap<BlockHash, u32>,
    /// Optional range of blocks indicating the completed header download up to the next checkpoint (start, end].
    completed_range: Option<(IndexedBlock, IndexedBlock)>,
}

/// Implements the Headers-First download strategy.
pub struct HeadersFirstDownloader<Block, Client> {
    client: Arc<Client>,
    header_verifier: HeaderVerifier<Block, Client>,
    peer_id: PeerId,
    state: State,
    block_downloader: BlockDownloader,
    downloaded_headers: DownloadedHeaders,
    /// Tracks the number of blocks downloaded from the current sync peer.
    downloaded_blocks_count: usize,
    last_locator_start: u32,
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
        header_verifier: HeaderVerifier<Block, Client>,
        peer_id: PeerId,
        target_block_number: u32,
        peer_store: Arc<dyn PeerStore>,
    ) -> (Self, SyncAction) {
        let mut headers_first_sync = Self {
            client,
            header_verifier,
            peer_id,
            state: State::Idle,
            downloaded_headers: DownloadedHeaders::default(),
            block_downloader: BlockDownloader::new(peer_store),
            downloaded_blocks_count: 0,
            last_locator_start: 0u32,
            target_block_number,
            _phantom: Default::default(),
        };
        let sync_action = headers_first_sync.headers_request_action();
        (headers_first_sync, sync_action)
    }

    pub(crate) fn sync_status(&self) -> SyncStatus {
        if self.block_downloader.queue_status.is_overloaded() {
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

    pub(crate) fn replaceable_sync_peer(&self) -> Option<PeerId> {
        (self.downloaded_blocks_count == 0).then_some(self.peer_id)
    }

    pub(crate) fn replace_sync_peer(&mut self, peer_id: PeerId, target_block_number: u32) {
        self.peer_id = peer_id;
        self.downloaded_blocks_count = 0;
        self.target_block_number = target_block_number;
    }

    pub(crate) fn block_downloader(&mut self) -> &mut BlockDownloader {
        &mut self.block_downloader
    }

    pub(crate) fn on_tick(&mut self) -> SyncAction {
        if matches!(self.state, State::RestartingHeaders) {
            return self.headers_request_action();
        }

        if let State::RestartingBlocks { start, end } = self.state {
            return self.start_block_download(start, end);
        }

        if self.block_downloader.queue_status.is_overloaded() {
            let is_ready = self
                .block_downloader
                .evaluate_queue_status(self.client.best_number())
                .is_ready();
            if is_ready {
                return self.resume_request();
            } else {
                return SyncAction::None;
            }
        }

        if self.block_downloader.is_stalled(self.peer_id) {
            return SyncAction::RestartSyncWithStalledPeer(self.peer_id);
        }

        SyncAction::None
    }

    // Resume blocks or headers request.
    fn resume_request(&mut self) -> SyncAction {
        match &mut self.state {
            State::DownloadingBlocks(BlockDownload::Batches {
                downloaded_batch,
                waiting,
                paused,
            }) if *paused => {
                *paused = false;

                if let Some(next_batch) = waiting.pop_front() {
                    tracing::debug!(
                        best_number = self.client.best_number(),
                        best_queued_number = self.block_downloader.best_queued_number,
                        "📦 Resumed downloading {} blocks in batches ({}/{})",
                        next_batch.len(),
                        *downloaded_batch + 1,
                        *downloaded_batch + 1 + waiting.len()
                    );
                    self.block_downloader
                        .requested_blocks
                        .clone_from(&next_batch);
                    self.blocks_request_action(next_batch)
                } else {
                    self.headers_request_action()
                }
            }
            _ => self.headers_request_action(),
        }
    }

    pub(crate) fn restart(&mut self, new_peer: PeerId, peer_best: u32) {
        self.peer_id = new_peer;
        self.downloaded_blocks_count = 0;
        self.last_locator_start = 0u32;
        self.target_block_number = peer_best;
        self.block_downloader.reset();
        if let Some((start, end)) = self.downloaded_headers.completed_range {
            self.state = State::RestartingBlocks { start, end };
        } else {
            self.downloaded_headers.headers.clear();
            self.state = State::RestartingHeaders;
        }
    }

    fn headers_request_action(&mut self) -> SyncAction {
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

        self.state = State::DownloadingHeaders { start, end };
        self.downloaded_headers = DownloadedHeaders {
            headers: IndexMap::with_capacity((end.number - start.number) as usize),
            completed_range: None,
        };

        SyncAction::Request(SyncRequest::Headers(LocatorRequest {
            locator_hashes,
            stop_hash: checkpoint.hash,
            to: self.peer_id,
        }))
    }

    // Handle `headers` message.
    //
    // `headers` are expected to contain at most 2000 entries, in ascending order. [b1, b2, b3, ..., b2000].
    pub(crate) fn on_headers(&mut self, headers: Vec<BitcoinHeader>, from: PeerId) -> SyncAction {
        if headers.len() > MAX_HEADERS_SIZE {
            return SyncAction::Disconnect(from, Error::TooManyHeaders);
        }

        let Some(first_header) = headers.first() else {
            // TODO: https://github.com/bitcoin/bitcoin/blob/6f9db1ebcab4064065ccd787161bf2b87e03cc1f/src/net_processing.cpp#L3014
            tracing::debug!("Received empty response of getheaders");
            return SyncAction::None;
        };

        let (start, end) = match &self.state {
            State::DownloadingHeaders { start, end } => (*start, *end),
            state => {
                tracing::debug!(%state, "Ignoring headers unexpected");
                return SyncAction::None;
            }
        };

        let mut prev_hash = first_header.prev_blockhash;

        let mut prev_number = if prev_hash == start.hash {
            start.number
        } else if let Some(block_number) = self.downloaded_headers.headers.get(&prev_hash).copied()
        {
            block_number
        } else if let Some(block_number) = self.client.block_number(prev_hash) {
            block_number
        } else {
            tracing::info!(
                ?first_header,
                best_number = ?self.client.info().best_number,
                "Cannot find the parent of the first header in headers, disconnecting"
            );
            self.state = State::Disconnecting;
            return SyncAction::Disconnect(self.peer_id, Error::MissingFirstHeaderParent);
        };

        for header in headers {
            if header.prev_blockhash != prev_hash {
                self.state = State::Disconnecting;
                return SyncAction::Disconnect(self.peer_id, Error::HeadersNotInAscendingOrder);
            }

            if !self.header_verifier.has_valid_proof_of_work(&header) {
                self.state = State::Disconnecting;
                return SyncAction::Disconnect(from, Error::BadProofOfWork(header.block_hash()));
            }

            let block_hash = header.block_hash();
            let block_number = prev_number + 1;

            // We can't convert the Bitcoin header to a Substrate header right now as creating a
            // Substrate header requires the full block data that is still missing.
            self.downloaded_headers
                .headers
                .insert(block_hash, block_number);

            prev_hash = block_hash;
            prev_number = block_number;
        }

        let final_block_number = prev_number;
        let target_block_number = end.number;
        let target_block_hash = end.hash;

        if final_block_number == target_block_number {
            self.downloaded_headers
                .completed_range
                .replace((start, end));
            self.start_block_download(start, end)
        } else {
            tracing::debug!("📄 Downloaded headers ({final_block_number}/{target_block_number})");

            SyncAction::Request(SyncRequest::Headers(LocatorRequest {
                locator_hashes: vec![prev_hash],
                stop_hash: target_block_hash,
                to: self.peer_id,
            }))
        }
    }

    // Fetch the block data of headers we have just downloaded.
    fn start_block_download(&mut self, start: IndexedBlock, end: IndexedBlock) -> SyncAction {
        // TODO: sync blocks from multiple peers in parallel.

        let best_number = self.client.best_number();

        let mut missing_blocks_count = 0usize;

        let missing_blocks =
            self.downloaded_headers
                .headers
                .iter()
                .filter_map(|(block_hash, block_number)| {
                    let block_hash = *block_hash;

                    if *block_number > best_number {
                        missing_blocks_count += 1;
                        return Some(block_hash);
                    }

                    if self.client.block_exists(block_hash) {
                        None
                    } else {
                        missing_blocks_count += 1;
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
                best_queued_number = self.block_downloader.best_queued_number,
                requested_blocks_count = get_data_msg.len(),
                missing_blocks_count,
                downloaded_headers_count = self.downloaded_headers.headers.len(),
                "Downloaded headers from {start} to {end}, requesting blocks",
            );

            self.state = State::DownloadingBlocks(BlockDownload::AllBlocks { start, end });

            get_data_msg
        } else {
            let batch_size = block_download_batch_size(end.number);
            let mut batches = missing_blocks
                .collect::<Vec<_>>()
                .chunks(batch_size)
                .map(|set| HashSet::from_iter(set.to_vec()))
                .collect::<VecDeque<HashSet<_>>>();

            let total_batches = batches.len();

            let Some(initial_batch) = batches.pop_front() else {
                tracing::warn!(
                    ?total_batches,
                    "Block download batches are empty, attempting new headers request"
                );
                return self.headers_request_action();
            };

            let old_requested = self
                .block_downloader
                .reset_requested_blocks(initial_batch.clone());

            assert!(
                old_requested.is_empty(),
                "There are still requested blocks not yet received: {old_requested:?}"
            );

            let blocks_request_count = initial_batch.len();

            tracing::debug!(
                best_number,
                best_queued_number = self.block_downloader.best_queued_number,
                missing_blocks_count,
                downloaded_headers_count = self.downloaded_headers.headers.len(),
                "Headers downloaded, requesting {blocks_request_count} blocks in batches (1/{total_batches})",
            );

            let get_data_msg =
                prepare_ordered_block_data_request(initial_batch, &self.downloaded_headers.headers);

            self.state = State::DownloadingBlocks(BlockDownload::Batches {
                downloaded_batch: 0,
                waiting: batches,
                paused: false,
            });

            get_data_msg
        };

        SyncAction::Request(SyncRequest::Data(get_data_msg, self.peer_id))
    }

    pub(crate) fn on_block(&mut self, block: BitcoinBlock, from: PeerId) -> SyncAction {
        let block_hash = block.block_hash();

        let block_download = match &mut self.state {
            State::DownloadingBlocks(block_download) => block_download,
            state => {
                // TODO: we may receive the blocks from a peer that has been considered as stalled,
                // should we try to cache and use such blocks since the bandwidth has been consumed
                // already?
                tracing::warn!(
                    ?state,
                    ?from,
                    current_sync_peer = ?self.peer_id,
                    "Not in the block download mode, dropping block {block_hash}",
                );
                return SyncAction::None;
            }
        };

        let receive_requested_block = self.block_downloader.on_block_response(block_hash);

        let parent_block_hash = block.header.prev_blockhash;

        let maybe_parent = self
            .block_downloader
            .block_number(parent_block_hash)
            .or_else(|| self.client.block_number(parent_block_hash));

        if let Some(parent_block_number) = maybe_parent {
            let block_number = parent_block_number + 1;

            tracing::trace!("Add pending block #{block_number},{block_hash}");

            self.block_downloader
                .add_block(block_number, block_hash, block, from);

            self.downloaded_blocks_count += 1;

            let should_request_more_headers = match block_download {
                BlockDownload::AllBlocks { start, end } => {
                    if end.hash == block_hash {
                        tracing::debug!("Downloaded blocks in ({start}, {end}]");
                        true
                    } else {
                        false
                    }
                }
                BlockDownload::Batches {
                    downloaded_batch,
                    waiting,
                    paused,
                } => {
                    if self.block_downloader.requested_blocks.is_empty() {
                        *downloaded_batch += 1;

                        let best_number = self.client.best_number();

                        if self
                            .block_downloader
                            .evaluate_queue_status(best_number)
                            .is_overloaded()
                        {
                            *paused = true;
                            return SyncAction::None;
                        }

                        if let Some(next_batch) = waiting.pop_front() {
                            tracing::debug!(
                                best_number,
                                best_queued_number = self.block_downloader.best_queued_number,
                                "📦 Downloaded {} blocks in batches ({}/{})",
                                next_batch.len(),
                                *downloaded_batch + 1,
                                *downloaded_batch + 1 + waiting.len()
                            );
                            self.block_downloader
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
                self.block_downloader.add_orphan_block(block_hash, block);
            } else {
                tracing::debug!("Discard unrequested orphan block {block_hash}");
            }

            SyncAction::None
        }
    }

    fn blocks_request_action(&self, blocks_to_download: HashSet<BlockHash>) -> SyncAction {
        let get_data_msg = prepare_ordered_block_data_request(
            blocks_to_download,
            &self.downloaded_headers.headers,
        );
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
            .block_downloader
            .evaluate_queue_status(best_number)
            .is_overloaded()
        {
            return SyncAction::None;
        }

        match crate::checkpoint::next_checkpoint(block_number + 1) {
            Some(checkpoint) => {
                self.state = State::DownloadingHeaders {
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
                    to: self.peer_id,
                }))
            }
            None => {
                // We have synced to the last checkpoint, now switch to the Blocks-First
                // sync to download the remaining blocks.
                tracing::debug!("No more checkpoint, switching to blocks-first sync");
                self.state = State::Completed;
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
            let block_number = downloaded_headers
                .get(&block_hash)
                .expect("Header must exist before downloading blocks in headers-first mode; qed");
            (block_number, block_hash)
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
