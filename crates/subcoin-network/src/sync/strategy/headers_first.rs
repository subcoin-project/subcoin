use crate::peer_store::PeerStore;
use crate::sync::block_downloader::BlockDownloader;
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

enum HeaderRequestOutcome {
    /// No checkpoints remain for requesting more headers.
    ExhaustedCheckpoint,
    /// A duplicate locator; skip additional requests.
    DuplicateLocator,
    /// Request new headers within the specified range.
    NewHeaders {
        payload: LocatorRequest,
        range: (IndexedBlock, IndexedBlock),
    },
}

struct HeaderDownloader<Block, Client> {
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

impl<Block, Client> HeaderDownloader<Block, Client>
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
    fn schedule_next_header_request_at(
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
            return HeaderRequestOutcome::DuplicateLocator;
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

        HeaderRequestOutcome::NewHeaders {
            payload,
            range: (start, end),
        }
    }
}

/// Implements the Headers-First download strategy.
pub struct HeadersFirstStrategy<Block, Client> {
    client: Arc<Client>,
    header_verifier: HeaderVerifier<Block, Client>,
    peer_id: PeerId,
    state: State,
    block_downloader: BlockDownloader,
    header_downloader: HeaderDownloader<Block, Client>,
    /// Tracks the number of blocks downloaded from the current sync peer.
    downloaded_blocks_count: usize,
    target_block_number: u32,
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
    ) -> (Self, SyncAction) {
        let best_number = client.best_number();

        let header_downloader = HeaderDownloader {
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
            block_downloader: BlockDownloader::new(peer_id, best_number, peer_store),
            header_downloader,
            downloaded_blocks_count: 0,
            target_block_number,
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
        self.block_downloader.peer_id = peer_id;
        self.header_downloader.peer_id = peer_id;
    }

    pub(crate) fn update_peer_best(&mut self, peer_id: PeerId, peer_best: u32) {
        if self.peer_id == peer_id {
            self.target_block_number = peer_best;
        }
    }

    pub(crate) fn block_downloader(&mut self) -> &mut BlockDownloader {
        &mut self.block_downloader
    }

    pub(crate) fn on_tick(&mut self) -> SyncAction {
        if matches!(self.state, State::RestartingHeaders) {
            return self.headers_request_action();
        }

        if let State::RestartingBlocks { start, end } = self.state {
            return self.start_block_download_on_header_download_completion(start, end);
        }

        if self.block_downloader.queue_status.is_overloaded() {
            let is_ready = self
                .block_downloader
                .evaluate_queue_status(self.client.best_number())
                .is_ready();

            if is_ready {
                // Resume the block download, otherwise schedule the next header request.
                match &mut self.state {
                    State::DownloadingBlocks(BlockDownload::Batches { paused }) if *paused => {
                        *paused = false;

                        if !self.block_downloader.missing_blocks.is_empty() {
                            tracing::debug!("Resumed downloading blocks");
                            return self.block_downloader.schedule_next_download_batch();
                        } else {
                            return self.headers_request_action();
                        }
                    }
                    _ => return self.headers_request_action(),
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

    pub(crate) fn restart(&mut self, new_peer: PeerId, peer_best: u32) {
        if let Some((start, end)) = self.header_downloader.completed_range {
            self.state = State::RestartingBlocks { start, end };
        } else {
            self.header_downloader.headers.clear();
            self.state = State::RestartingHeaders;
        }
        self.peer_id = new_peer;
        self.target_block_number = peer_best;
        self.downloaded_blocks_count = 0;
        self.block_downloader.reset();
        self.block_downloader.peer_id = new_peer;
        self.header_downloader.peer_id = new_peer;
        self.header_downloader.last_locator_request_start = 0u32;
    }

    fn headers_request_action(&mut self) -> SyncAction {
        let best_number = self.client.best_number();
        let best_hash = self
            .client
            .block_hash(best_number)
            .expect("Best hash must exist; qed");
        let header_request_outcome = self
            .header_downloader
            .schedule_next_header_request_at(best_number, best_hash);
        self.process_header_request_outcome(header_request_outcome)
    }

    fn process_header_request_outcome(
        &mut self,
        header_request_outcome: HeaderRequestOutcome,
    ) -> SyncAction {
        match header_request_outcome {
            HeaderRequestOutcome::DuplicateLocator => SyncAction::None,
            HeaderRequestOutcome::ExhaustedCheckpoint => {
                tracing::debug!("No more checkpoints, switching to blocks-first sync");
                self.state = State::Completed;
                SyncAction::SwitchToBlocksFirstSync
            }
            HeaderRequestOutcome::NewHeaders {
                payload,
                range: (start, end),
            } => {
                tracing::debug!("Downloading headers ({start}, {end}]");
                self.state = State::DownloadingHeaders { start, end };
                SyncAction::Request(SyncRequest::Headers(payload))
            }
        }
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
        } else if let Some(block_number) = self.header_downloader.headers.get(&prev_hash).copied() {
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
            self.header_downloader
                .headers
                .insert(block_hash, block_number);

            prev_hash = block_hash;
            prev_number = block_number;
        }

        let final_block_number = prev_number;
        let target_block_number = end.number;
        let target_block_hash = end.hash;

        if final_block_number == target_block_number {
            self.header_downloader.completed_range.replace((start, end));
            self.start_block_download_on_header_download_completion(start, end)
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
    fn start_block_download_on_header_download_completion(
        &mut self,
        start: IndexedBlock,
        end: IndexedBlock,
    ) -> SyncAction {
        // TODO: sync blocks from multiple peers in parallel.

        let best_number = self.client.best_number();

        let missing_blocks = self.header_downloader.compute_missing_blocks(best_number);

        if missing_blocks.is_empty() {
            tracing::debug!("No missing blocks, starting new headers request");
            return self.headers_request_action();
        }

        // If the sync peer is running from local, the bandwidth is not a bottleneck,
        // simply request all blocks at once.
        let download_all_blocks_in_one_request = is_local_address(&self.peer_id);

        if download_all_blocks_in_one_request {
            tracing::debug!(
                best_number,
                best_queued_number = self.block_downloader.best_queued_number,
                downloaded_headers_count = self.header_downloader.headers.len(),
                "Headers downloaded, starting to download {} blocks from {start} to {end}",
                missing_blocks.len()
            );

            self.state = State::DownloadingBlocks(BlockDownload::AllBlocks { start, end });

            SyncAction::Request(SyncRequest::Data(
                missing_blocks
                    .into_iter()
                    .map(Inventory::Block)
                    .collect::<Vec<_>>(),
                self.peer_id,
            ))
        } else {
            self.state = State::DownloadingBlocks(BlockDownload::Batches { paused: false });
            self.block_downloader.set_missing_blocks(missing_blocks);
            self.block_downloader.schedule_next_download_batch()
        }
    }

    pub(crate) fn on_block(&mut self, block: BitcoinBlock, from: PeerId) -> SyncAction {
        let block_hash = block.block_hash();

        match &self.state {
            State::DownloadingBlocks(_block_download) => {}
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

            self.downloaded_blocks_count += 1;

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
            State::DownloadingBlocks(block_download) => block_download,
            _state => unreachable!("Must be DownloadingBlocks as checked; qed"),
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
                if self.block_downloader.requested_blocks.is_empty() {
                    if !self.block_downloader.missing_blocks.is_empty() {
                        if self
                            .block_downloader
                            .evaluate_queue_status(self.client.best_number())
                            .is_overloaded()
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
                .is_overloaded()
            {
                return SyncAction::None;
            }

            let header_request_outcome = self
                .header_downloader
                .schedule_next_header_request_at(block_number, block_hash);
            self.process_header_request_outcome(header_request_outcome)
        } else {
            SyncAction::None
        }
    }
}

fn is_local_address(addr: &PeerId) -> bool {
    match addr.ip() {
        IpAddr::V4(ipv4) => ipv4.is_loopback() || ipv4.is_private() || ipv4.is_unspecified(),
        IpAddr::V6(_ipv6) => false,
    }
}
