use crate::peer_store::PeerStore;
use crate::sync::block_downloader::BlockDownloader;
use crate::sync::{LocatorRequest, SyncAction};
use crate::{Error, MemoryConfig, PeerId, SyncStatus};
use bitcoin::blockdata::block::Header as BitcoinHeader;
use bitcoin::p2p::message_blockdata::Inventory;
use bitcoin::{Block as BitcoinBlock, BlockHash};
use indexmap::IndexMap;
use sc_client_api::AuxStore;
use sc_consensus_nakamoto::HeaderVerifier;
use sp_blockchain::HeaderBackend;
use sp_runtime::traits::Block as BlockT;
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
        /// Is the blocks download paused?
        paused: bool,
    },
}

/// Represents the header prefetch pipeline for downloading the next checkpoint in advance.
#[derive(Default)]
enum HeaderPrefetch<Block, Client> {
    /// No prefetch in progress.
    #[default]
    None,
    /// Prefetch initiated, need to send getheaders request.
    Initiated {
        /// Target checkpoint we're prefetching headers for.
        checkpoint: IndexedBlock,
        /// Anchor block (last block of current checkpoint) that prefetched headers must chain to.
        /// This stays stable across all partial responses for this prefetch.
        anchor: IndexedBlock,
        /// Header requester for managing the prefetch download.
        requester: HeaderRequester<Block, Client>,
    },
    /// Getheaders request sent, awaiting response.
    Fetching {
        /// Target checkpoint we're prefetching headers for.
        checkpoint: IndexedBlock,
        /// Anchor block (last block of current checkpoint) that prefetched headers must chain to.
        /// This stays stable across all partial responses for this prefetch.
        anchor: IndexedBlock,
        /// Header requester for managing the prefetch download.
        requester: HeaderRequester<Block, Client>,
    },
    /// Prefetched headers are ready and verified.
    Ready {
        headers: IndexMap<BlockHash, u32>,
        start: IndexedBlock,
        end: IndexedBlock,
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
    FetchingHeaders {
        start: IndexedBlock,
        end: IndexedBlock,
    },
    /// Actively downloading blocks corresponding to previously downloaded headers (start, end].
    FetchingBlockData(BlockDownload),
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
            Self::FetchingHeaders { start, end } => {
                write!(f, "FetchingHeaders {{ start: {start}, end: {end} }}")
            }
            Self::FetchingBlockData(_range) => write!(f, "FetchingBlockData"),
            Self::Completed => write!(f, "Completed"),
        }
    }
}

enum HeaderRequestOutcome {
    /// No checkpoints remain for requesting more headers.
    ExhaustedCheckpoint,
    /// A duplicate locator; skip additional requests.
    RepeatedRequest,
    /// Request new headers within the specified range.
    NewGetHeaders {
        payload: LocatorRequest,
        range: (IndexedBlock, IndexedBlock),
    },
}

struct HeaderRequester<Block, Client> {
    client: Arc<Client>,
    peer_id: PeerId,
    /// Ordered map of headers, where the key is the block hash and the value is the block number.
    ///
    /// Keep the headers ordered so that fetching the blocks orderly later is possible.
    headers: IndexMap<BlockHash, u32>,
    /// Optional range of blocks indicating the completed header download up to the next checkpoint (start, end].
    completed_range: Option<(IndexedBlock, IndexedBlock)>,
    last_locator_request_start: u32,
    _phantom: PhantomData<Block>,
}

impl<Block, Client> HeaderRequester<Block, Client>
where
    Block: BlockT,
    Client: HeaderBackend<Block> + AuxStore,
{
    /// Computes the list of missing blocks after the predefined set of headers are all downloaded.
    fn compute_missing_blocks(&mut self, best_number: u32) -> Vec<BlockHash> {
        assert!(
            self.completed_range.is_some(),
            "`compute_missing_blocks()` should be only invoked after the completion of block list download.",
        );

        let mut missing_blocks = self
            .headers
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
            })
            .collect::<Vec<_>>();

        // Sort to avoid too many orphan blocks.
        missing_blocks.sort_by_key(|block_hash| {
            self.headers
                .get(block_hash)
                .expect("Header must be available; qed ")
        });

        missing_blocks
    }

    /// Prepares a request for the next set of headers based on the specified
    /// block number and the next checkpoint.
    ///
    /// Ensures no duplicate locators are sent consecutively, and stores the last
    /// requested locator start position.
    fn next_header_request_at(
        &mut self,
        block_number: u32,
        block_hash: BlockHash,
    ) -> HeaderRequestOutcome {
        let Some(checkpoint) = crate::checkpoint::next_checkpoint(block_number + 1) else {
            return HeaderRequestOutcome::ExhaustedCheckpoint;
        };

        let start = IndexedBlock {
            number: block_number,
            hash: block_hash,
        };

        let end = checkpoint;

        // Ignore the back-to-back duplicate locators.
        if block_number > 0 && block_number == self.last_locator_request_start {
            return HeaderRequestOutcome::RepeatedRequest;
        }

        self.last_locator_request_start = block_number;

        self.headers = IndexMap::with_capacity((end.number - start.number) as usize);
        self.completed_range = None;

        let locator_hashes = self
            .client
            .block_locator(Some(block_number), |_height: u32| None)
            .locator_hashes;

        let payload = LocatorRequest {
            locator_hashes,
            stop_hash: checkpoint.hash,
            to: self.peer_id,
        };

        HeaderRequestOutcome::NewGetHeaders {
            payload,
            range: (start, end),
        }
    }
}

/// Prefetch statistics for tracking hit/miss ratio.
#[derive(Debug, Clone, Default)]
pub(crate) struct PrefetchStats {
    pub(crate) hits: u64,
    pub(crate) misses: u64,
}

/// Implements the Headers-First download strategy.
pub struct HeadersFirstStrategy<Block, Client> {
    state: State,
    client: Arc<Client>,
    peer_id: PeerId,
    header_verifier: HeaderVerifier<Block, Client>,
    block_downloader: BlockDownloader,
    header_requester: HeaderRequester<Block, Client>,
    target_block_number: u32,
    header_prefetch: HeaderPrefetch<Block, Client>,
    prefetch_stats: PrefetchStats,
}

/// Process and validate a sequence of headers.
///
/// Returns the final block hash and number after processing all headers.
fn validate_and_store_headers<Block, Client>(
    header_verifier: &HeaderVerifier<Block, Client>,
    headers: Vec<BitcoinHeader>,
    prev_hash: BlockHash,
    prev_number: u32,
    header_map: &mut IndexMap<BlockHash, u32>,
) -> Result<(BlockHash, u32), Error>
where
    Block: BlockT,
    Client: HeaderBackend<Block> + AuxStore,
{
    let mut current_hash = prev_hash;
    let mut current_number = prev_number;

    for header in headers {
        if header.prev_blockhash != current_hash {
            return Err(Error::HeadersNotInAscendingOrder);
        }

        if !header_verifier.has_valid_proof_of_work(&header) {
            return Err(Error::BadProofOfWork(header.block_hash()));
        }

        let block_hash = header.block_hash();
        let block_number = current_number + 1;

        // We can't convert the Bitcoin header to a Substrate header right now as creating a
        // Substrate header requires the full block data that is still missing.
        header_map.insert(block_hash, block_number);

        current_hash = block_hash;
        current_number = block_number;
    }

    Ok((current_hash, current_number))
}

impl<Block, Client> HeadersFirstStrategy<Block, Client>
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
        memory_config: MemoryConfig,
    ) -> (Self, SyncAction) {
        let best_number = client.best_number();

        let header_requester = HeaderRequester {
            client: client.clone(),
            peer_id,
            last_locator_request_start: 0u32,
            headers: IndexMap::new(),
            completed_range: None,
            _phantom: Default::default(),
        };

        let mut headers_first_sync = Self {
            client,
            header_verifier,
            peer_id,
            state: State::Idle,
            block_downloader: BlockDownloader::new(peer_id, best_number, peer_store, memory_config),
            header_requester,
            target_block_number,
            header_prefetch: HeaderPrefetch::None,
            prefetch_stats: PrefetchStats::default(),
        };

        let sync_action = headers_first_sync.header_request_action();

        (headers_first_sync, sync_action)
    }

    pub(crate) fn sync_status(&self) -> SyncStatus {
        if self.block_downloader.queue_status.is_saturated() {
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

    pub(crate) fn can_swap_sync_peer(&self) -> Option<PeerId> {
        (self.block_downloader.downloaded_blocks_count == 0).then_some(self.peer_id)
    }

    pub(crate) fn swap_sync_peer(&mut self, peer_id: PeerId, target_block_number: u32) {
        self.peer_id = peer_id;
        self.target_block_number = target_block_number;
        self.block_downloader.peer_id = peer_id;
        self.block_downloader.downloaded_blocks_count = 0;
        self.header_requester.peer_id = peer_id;
        // Drop prefetch state on peer change to avoid applying headers from stale peer
        self.header_prefetch = HeaderPrefetch::None;
    }

    pub(crate) fn set_peer_best(&mut self, peer_id: PeerId, peer_best: u32) {
        if self.peer_id == peer_id {
            self.target_block_number = peer_best;
        }
    }

    pub(crate) fn block_downloader(&mut self) -> &mut BlockDownloader {
        &mut self.block_downloader
    }

    pub(crate) fn on_tick(&mut self) -> SyncAction {
        if matches!(self.state, State::RestartingHeaders) {
            return self.header_request_action();
        }

        if let State::RestartingBlocks { start, end } = self.state {
            return self.start_block_download_on_header_download_completion(start, end);
        }

        if self.client.best_number() == self.target_block_number {
            self.state = State::Completed;
            return SyncAction::SetIdle;
        }

        // Send prefetch getheaders request if in Initiated state
        if matches!(self.state, State::FetchingBlockData(_))
            && matches!(self.header_prefetch, HeaderPrefetch::Initiated { .. })
        {
            return self.send_prefetch_getheaders();
        }

        if self.block_downloader.queue_status.is_saturated() {
            let is_ready = self
                .block_downloader
                .evaluate_queue_status(self.client.best_number())
                .is_ready();

            if is_ready {
                // Resume the block download, otherwise schedule the next header request.
                match &mut self.state {
                    State::FetchingBlockData(BlockDownload::Batches { paused }) if *paused => {
                        *paused = false;

                        if !self.block_downloader.missing_blocks.is_empty() {
                            tracing::debug!(
                                missing_blocks = self.block_downloader.missing_blocks.len(),
                                "Resumed downloading blocks",
                            );
                            return self.block_downloader.schedule_next_download_batch();
                        } else {
                            return self.header_request_action();
                        }
                    }
                    _ => return self.header_request_action(),
                }
            } else {
                return SyncAction::None;
            }
        }

        if let Some(stalled_peer) = self.block_downloader.has_stalled() {
            return SyncAction::RestartSyncWithStalledPeer(stalled_peer);
        }

        SyncAction::None
    }

    pub(crate) fn restart(&mut self, new_peer: PeerId, target_block_number: u32) {
        if let Some((start, end)) = self.header_requester.completed_range {
            self.state = State::RestartingBlocks { start, end };
        } else {
            self.header_requester.headers.clear();
            self.state = State::RestartingHeaders;
        }
        self.peer_id = new_peer;
        self.target_block_number = target_block_number;
        self.block_downloader.restart(new_peer);
        self.header_requester.peer_id = new_peer;
        self.header_requester.last_locator_request_start = 0u32;
        // Drop prefetch state on restart to avoid applying headers from stale peer
        self.header_prefetch = HeaderPrefetch::None;
    }

    fn header_request_action(&mut self) -> SyncAction {
        let best_number = self.client.best_number();
        let best_hash = self
            .client
            .block_hash(best_number)
            .expect("Best hash must exist; qed");
        let header_request_outcome = self
            .header_requester
            .next_header_request_at(best_number, best_hash);
        self.process_header_request_outcome(header_request_outcome)
    }

    fn process_header_request_outcome(
        &mut self,
        header_request_outcome: HeaderRequestOutcome,
    ) -> SyncAction {
        match header_request_outcome {
            HeaderRequestOutcome::RepeatedRequest => SyncAction::None,
            HeaderRequestOutcome::ExhaustedCheckpoint => {
                tracing::debug!("No more checkpoints, switching to blocks-first sync");
                self.state = State::Completed;
                SyncAction::SwitchToBlocksFirstSync
            }
            HeaderRequestOutcome::NewGetHeaders {
                payload,
                range: (start, end),
            } => {
                tracing::debug!("Downloading headers ({start}, {end}]");
                self.state = State::FetchingHeaders { start, end };
                SyncAction::get_headers(payload)
            }
        }
    }

    /// Send a prefetch getheaders request and transition from Initiated → Fetching.
    ///
    /// Precondition: `header_prefetch` must be in `Initiated` state.
    fn send_prefetch_getheaders(&mut self) -> SyncAction {
        let HeaderPrefetch::Initiated {
            checkpoint,
            anchor,
            mut requester,
        } = std::mem::take(&mut self.header_prefetch)
        else {
            unreachable!("Caller must check header_prefetch is Initiated");
        };

        requester.last_locator_request_start = anchor.number;

        // Create locator that searches downloaded headers first, then falls back to client
        let header_requester_headers = &self.header_requester.headers;
        let locator_hashes = self
            .client
            .block_locator(Some(anchor.number), |height| {
                // Search downloaded-but-not-imported headers first
                header_requester_headers
                    .iter()
                    .find(|(_, num)| **num == height)
                    .map(|(hash, _)| *hash)
                    .or_else(|| self.client.block_hash(height))
            })
            .locator_hashes;

        tracing::debug!(
            "📡 Sending prefetch getheaders for range (#{}, #{}]",
            anchor.number,
            checkpoint.number
        );

        // Transition to Fetching state
        self.header_prefetch = HeaderPrefetch::Fetching {
            checkpoint,
            anchor,
            requester,
        };

        SyncAction::get_headers(LocatorRequest {
            locator_hashes,
            stop_hash: checkpoint.hash,
            to: self.peer_id,
        })
    }

    // Handle prefetch `headers` message.
    fn on_prefetch_headers(&mut self, headers: Vec<BitcoinHeader>, _from: PeerId) -> SyncAction {
        // Check if headers is non-empty first (before any state manipulation)
        let Some(first_header) = headers.first() else {
            tracing::debug!("Received empty prefetch headers response");
            return SyncAction::None;
        };

        // Only accept headers when in Fetching state (request must have been sent)
        if !matches!(self.header_prefetch, HeaderPrefetch::Fetching { .. }) {
            tracing::warn!("Received prefetch headers in unexpected state (expected Fetching)");
            return SyncAction::None;
        }

        // Extract checkpoint, anchor, and requester
        let HeaderPrefetch::Fetching {
            checkpoint,
            anchor,
            mut requester,
        } = std::mem::take(&mut self.header_prefetch)
        else {
            unreachable!("Just checked it's Fetching");
        };

        // Find the expected start block for prefetch headers
        let prev_hash = first_header.prev_blockhash;
        let prev_number = if let Some(block_number) = requester.headers.get(&prev_hash).copied() {
            block_number
        } else if let Some(block_number) = self.header_requester.headers.get(&prev_hash).copied() {
            block_number
        } else if let Some(block_number) = self.client.block_number(prev_hash) {
            block_number
        } else {
            tracing::info!(
                ?first_header,
                "Cannot find parent of first prefetch header, resetting prefetch"
            );
            // State already reset to None at the top
            return SyncAction::None;
        };

        // Verify and store prefetch headers
        let (final_block_hash, final_block_number) = match validate_and_store_headers(
            &self.header_verifier,
            headers,
            prev_hash,
            prev_number,
            &mut requester.headers,
        ) {
            Ok((hash, number)) => (hash, number),
            Err(err) => {
                tracing::warn!("Prefetch header validation failed: {err:?}, resetting prefetch");
                // State already reset to None at the top
                return SyncAction::None;
            }
        };

        // Update progress time to prevent stall detection during prefetch header download
        self.block_downloader.touch_progress();

        if final_block_number == checkpoint.number {
            // Prefetch headers complete - mark as Ready with validation
            // Use the stored anchor (which is stable across all partial responses)
            let start = anchor;
            let end = IndexedBlock {
                hash: final_block_hash,
                number: final_block_number,
            };

            let prefetch_headers = requester.headers;
            let headers_count = prefetch_headers.len();

            self.header_prefetch = HeaderPrefetch::Ready {
                headers: prefetch_headers,
                start,
                end,
            };

            tracing::debug!(
                "📄 Prefetch headers complete (#{}, #{}] ({} headers)",
                start.number,
                end.number,
                headers_count
            );
            SyncAction::None
        } else {
            tracing::debug!(
                "📄 Prefetch headers partial ({final_block_number}/{})",
                checkpoint.number
            );

            // Request more prefetch headers with proper locator, using the original checkpoint hash
            let prefetch_headers = &requester.headers;
            let locator_hashes = self
                .client
                .block_locator(Some(final_block_number), |height| {
                    // Search downloaded prefetch headers first, then fall back to client
                    prefetch_headers
                        .iter()
                        .find(|(_, num)| **num == height)
                        .map(|(hash, _)| *hash)
                        .or_else(|| self.client.block_hash(height))
                })
                .locator_hashes;

            // Update last_locator_request_start to prevent duplicate requests from on_tick
            requester.last_locator_request_start = final_block_number;

            // Restore state with updated requester
            self.header_prefetch = HeaderPrefetch::Fetching {
                checkpoint,
                anchor,
                requester,
            };

            SyncAction::get_headers(LocatorRequest {
                locator_hashes,
                stop_hash: checkpoint.hash,
                to: self.peer_id,
            })
        }
    }

    // Handle `headers` message.
    //
    // `headers` are expected to contain at most 2000 entries, in ascending order. [b1, b2, b3, ..., b2000].
    pub(crate) fn on_headers(&mut self, headers: Vec<BitcoinHeader>, from: PeerId) -> SyncAction {
        if headers.len() > MAX_HEADERS_SIZE {
            return SyncAction::DisconnectPeer(from, Error::TooManyHeaders);
        }

        let Some(first_header) = headers.first() else {
            // TODO: https://github.com/bitcoin/bitcoin/blob/6f9db1ebcab4064065ccd787161bf2b87e03cc1f/src/net_processing.cpp#L3014
            tracing::debug!("Received empty response of getheaders");
            return SyncAction::None;
        };

        // Check if these are prefetch headers (when we're downloading blocks and prefetch is in progress)
        if matches!(self.state, State::FetchingBlockData(_))
            && matches!(self.header_prefetch, HeaderPrefetch::Fetching { .. })
        {
            return self.on_prefetch_headers(headers, from);
        }

        let (start, end) = match &self.state {
            State::FetchingHeaders { start, end } => (*start, *end),
            state => {
                tracing::debug!(%state, "Ignoring headers unexpected");
                return SyncAction::None;
            }
        };

        let prev_hash = first_header.prev_blockhash;

        let prev_number = if prev_hash == start.hash {
            start.number
        } else if let Some(block_number) = self.header_requester.headers.get(&prev_hash).copied() {
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
            return SyncAction::DisconnectPeer(self.peer_id, Error::MissingFirstHeaderParent);
        };

        let (final_block_hash, final_block_number) = match validate_and_store_headers(
            &self.header_verifier,
            headers,
            prev_hash,
            prev_number,
            &mut self.header_requester.headers,
        ) {
            Ok((hash, number)) => (hash, number),
            Err(err) => {
                self.state = State::Disconnecting;
                return SyncAction::DisconnectPeer(from, err);
            }
        };

        // Update progress time to prevent stall detection during header download
        self.block_downloader.touch_progress();

        let target_block_number = end.number;
        let target_block_hash = end.hash;

        if final_block_number == target_block_number {
            self.header_requester.completed_range.replace((start, end));
            self.start_block_download_on_header_download_completion(start, end)
        } else {
            tracing::debug!("📄 Downloaded headers ({final_block_number}/{target_block_number})");

            SyncAction::get_headers(LocatorRequest {
                locator_hashes: vec![final_block_hash],
                stop_hash: target_block_hash,
                to: self.peer_id,
            })
        }
    }

    // Fetch the block data of headers we have just downloaded.
    fn start_block_download_on_header_download_completion(
        &mut self,
        start: IndexedBlock,
        end: IndexedBlock,
    ) -> SyncAction {
        // TODO: sync blocks from multiple peers in parallel.

        let best_number = self.client.best_number();

        let missing_blocks = self.header_requester.compute_missing_blocks(best_number);

        if missing_blocks.is_empty() {
            tracing::debug!("No missing blocks, starting new headers request");
            return self.header_request_action();
        }

        // If the sync peer is running from local, the bandwidth is not a bottleneck,
        // simply request all blocks at once.
        let download_all_blocks_in_one_request = is_local_address(&self.peer_id);

        if download_all_blocks_in_one_request {
            tracing::debug!(
                best_number,
                best_queued_number = self.block_downloader.best_queued_number,
                downloaded_headers_count = self.header_requester.headers.len(),
                "Headers downloaded, starting to download {} blocks in range ({start}, {end}]",
                missing_blocks.len()
            );

            self.state = State::FetchingBlockData(BlockDownload::AllBlocks { start, end });

            self.try_initiate_header_prefetch_for_next_checkpoint(end.number, end.hash);

            let inv = missing_blocks
                .into_iter()
                .map(Inventory::Block)
                .collect::<Vec<_>>();

            SyncAction::get_data(inv, self.peer_id)
        } else {
            self.state = State::FetchingBlockData(BlockDownload::Batches { paused: false });
            self.block_downloader.set_missing_blocks(missing_blocks);

            self.try_initiate_header_prefetch_for_next_checkpoint(end.number, end.hash);

            self.block_downloader.schedule_next_download_batch()
        }
    }

    pub(crate) fn on_block(&mut self, block: BitcoinBlock, from: PeerId) -> SyncAction {
        let block_hash = block.block_hash();

        match &self.state {
            State::FetchingBlockData(_block_download) => {}
            state => {
                // TODO: we may receive the blocks from a peer that has been considered as stalled,
                // should we try to cache and use such blocks since the bandwidth has been consumed
                // already?
                tracing::warn!(
                    ?state,
                    current_sync_peer = ?self.peer_id,
                    "Not in the block download mode, dropping block {block_hash} from {from:?}",
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

            self.schedule_block_download(block_number, block_hash)
        } else {
            if receive_requested_block {
                self.block_downloader.add_orphan_block(block_hash, block);
            } else {
                tracing::debug!("Discard unrequested orphan block {block_hash}");
            }

            SyncAction::None
        }
    }

    fn schedule_block_download(&mut self, block_number: u32, block_hash: BlockHash) -> SyncAction {
        let block_download = match &mut self.state {
            State::FetchingBlockData(block_download) => block_download,
            _state => unreachable!("Must be FetchingBlockData as checked; qed"),
        };

        let should_request_more_headers = match block_download {
            BlockDownload::AllBlocks { start, end } => {
                if end.hash == block_hash {
                    tracing::debug!("Downloaded blocks in ({start}, {end}]");
                    true
                } else {
                    false
                }
            }
            BlockDownload::Batches { paused } => {
                // Check if we should pipeline (request next batch before current is complete)
                if self.block_downloader.should_pipeline()
                    && !self
                        .block_downloader
                        .evaluate_queue_status(self.client.best_number())
                        .is_saturated()
                {
                    tracing::trace!(
                        "Pipelining: requesting next batch while current batch in progress"
                    );
                    return self.block_downloader.schedule_next_download_batch();
                }

                if self.block_downloader.requested_blocks.is_empty() {
                    if !self.block_downloader.missing_blocks.is_empty() {
                        if self
                            .block_downloader
                            .evaluate_queue_status(self.client.best_number())
                            .is_saturated()
                        {
                            *paused = true;
                            return SyncAction::None;
                        }

                        // Request next batch of blocks.
                        return self.block_downloader.schedule_next_download_batch();
                    }

                    tracing::debug!("Downloaded checkpoint block #{block_number},{block_hash}");

                    true
                } else {
                    false
                }
            }
        };

        if should_request_more_headers {
            if self
                .block_downloader
                .evaluate_queue_status(self.client.best_number())
                .is_saturated()
            {
                return SyncAction::None;
            }

            // Try to use prefetched headers if available
            if let Some(action) = self.try_use_prefetched_headers(block_hash) {
                return action;
            }

            // No prefetch available or chain mismatch - request headers normally (MISS)
            self.prefetch_stats.misses += 1;
            let header_request_outcome = self
                .header_requester
                .next_header_request_at(block_number, block_hash);

            self.process_header_request_outcome(header_request_outcome)
        } else {
            SyncAction::None
        }
    }

    /// Try to use prefetched headers if they're ready and chain correctly.
    ///
    /// Returns `Some(SyncAction)` if prefetch hits, `None` otherwise.
    fn try_use_prefetched_headers(&mut self, block_hash: BlockHash) -> Option<SyncAction> {
        let HeaderPrefetch::Ready {
            headers: _,
            start,
            end,
        } = &self.header_prefetch
        else {
            return None;
        };

        // Validate chaining: last block of current checkpoint must match first block of prefetch
        if block_hash != start.hash {
            // Chain mismatch - discard prefetch
            tracing::warn!(
                "Prefetch chain mismatch: expected start {}, got {block_hash}. Discarding prefetch.",
                start.hash,
            );
            self.header_prefetch = HeaderPrefetch::None;
            return None;
        }

        // Prefetch hit! Use the prefetched headers immediately
        self.prefetch_stats.hits += 1;
        tracing::debug!(
            "🎯 Prefetch HIT: Using prefetched headers for (#{}, #{}]",
            start.number,
            end.number
        );

        // Take ownership of prefetch headers
        let HeaderPrefetch::Ready {
            headers: prefetch_headers,
            start: prefetch_start,
            end: prefetch_end,
        } = std::mem::take(&mut self.header_prefetch)
        else {
            tracing::error!("Prefetch state changed between check and use");
            return None;
        };

        // Promote prefetch to active
        self.header_requester.headers = prefetch_headers;
        self.header_requester.completed_range = Some((prefetch_start, prefetch_end));

        // Start downloading blocks for the prefetched checkpoint
        let sync_action =
            self.start_block_download_on_header_download_completion(prefetch_start, prefetch_end);

        // Initiate prefetch for the next checkpoint while downloading these blocks
        self.try_initiate_header_prefetch_for_next_checkpoint(
            prefetch_end.number,
            prefetch_end.hash,
        );

        Some(sync_action)
    }

    // Initiate prefetch for the next checkpoint
    fn try_initiate_header_prefetch_for_next_checkpoint(
        &mut self,
        block_number: u32,
        block_hash: BlockHash,
    ) {
        // Only prefetch if not already in progress and we haven't exceeded checkpoints
        if !matches!(self.header_prefetch, HeaderPrefetch::None) {
            return;
        }

        let Some(next_checkpoint) = crate::checkpoint::next_checkpoint(block_number + 1) else {
            // No more checkpoints
            return;
        };

        // Compute the anchor block (parent of the first header we'll prefetch)
        // This is the last block of the current checkpoint
        let anchor = IndexedBlock {
            hash: block_hash,
            number: block_number,
        };

        // Create a new HeaderRequester for prefetch
        let requester = HeaderRequester {
            client: self.client.clone(),
            peer_id: self.peer_id,
            last_locator_request_start: 0u32,
            headers: IndexMap::new(),
            completed_range: None,
            _phantom: Default::default(),
        };

        self.header_prefetch = HeaderPrefetch::Initiated {
            checkpoint: next_checkpoint,
            anchor,
            requester,
        };

        tracing::debug!(
            "🔮 Initiating header prefetch for range (#{}, #{}]",
            anchor.number,
            next_checkpoint.number
        );
    }
}

fn is_local_address(addr: &PeerId) -> bool {
    match addr.ip() {
        IpAddr::V4(ipv4) => ipv4.is_loopback() || ipv4.is_private() || ipv4.is_unspecified(),
        IpAddr::V6(_ipv6) => false,
    }
}
