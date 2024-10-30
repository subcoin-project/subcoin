use crate::orphan_blocks_pool::OrphanBlocksPool;
use crate::peer_store::PeerStore;
use crate::sync::{SyncAction, SyncRequest};
use crate::PeerId;
use bitcoin::p2p::message_blockdata::Inventory;
use bitcoin::{Block as BitcoinBlock, BlockHash};
use sc_consensus::BlockImportError;
use sc_consensus_nakamoto::ImportManyBlocksResult;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Interval for logging when the block import queue is too busy.
const BUSY_QUEUE_LOG_INTERVAL: Duration = Duration::from_secs(5);

/// Manages queued blocks.
#[derive(Default, Debug, Clone)]
pub(crate) struct QueuedBlocks {
    hash2number: HashMap<BlockHash, u32>,
    number2hash: HashMap<u32, BlockHash>,
}

impl QueuedBlocks {
    pub(crate) fn block_hash(&self, number: u32) -> Option<BlockHash> {
        self.number2hash.get(&number).copied()
    }

    pub(crate) fn block_number(&self, hash: BlockHash) -> Option<u32> {
        self.hash2number.get(&hash).copied()
    }

    fn insert(&mut self, number: u32, hash: BlockHash) {
        self.hash2number.insert(hash, number);
        self.number2hash.insert(number, hash);
    }

    fn remove(&mut self, hash: &BlockHash) {
        if let Some(number) = self.hash2number.remove(hash) {
            self.number2hash.remove(&number);
        }
    }

    fn clear(&mut self) {
        self.hash2number.clear();
        self.number2hash.clear();
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ImportQueueStatus {
    /// Queue is ready to accept more blocks for import.
    Ready,
    /// The queue is overloaded and cannot accept more blocks at the moment.
    Overloaded,
}

impl ImportQueueStatus {
    pub(crate) fn is_overloaded(&self) -> bool {
        matches!(self, Self::Overloaded)
    }

    pub(crate) fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }
}

/// Responsible for downloading blocks from a specific peer.
///
/// This struct keeps track of:
/// - Blocks that have been requested from the network but not yet received.
/// - Blocks that have been downloaded but not yet sent to the import queue.
/// - Blocks that are currently in the import queue awaiting processing.
/// - The highest block number that is either in the queue or pending to be queued.
///
/// [`BlockDownloader`] is designed to be used in both Blocks-First and
/// Headers-First sync strategies, providing a common component for managing
/// the state of blocks during the sync process.
#[derive(Clone)]
pub(crate) struct BlockDownloader {
    pub(crate) peer_id: PeerId,
    /// Blocks awaiting to be downloaded.
    pub(crate) missing_blocks: Vec<BlockHash>,
    /// A set of block hashes that have been requested from the network.
    ///
    /// This helps in tracking which blocks are pending download.
    pub(crate) requested_blocks: HashSet<BlockHash>,
    /// A vector of blocks that have been downloaded from the network.
    ///
    /// These blocks are ready to be sent to the import queue.
    pub(crate) downloaded_blocks: Vec<BitcoinBlock>,
    /// A map of block hashes to their respective heights in the import queue.
    /// This helps in tracking which blocks are currently being processed in the import queue.
    pub(crate) blocks_in_queue: HashMap<BlockHash, u32>,
    /// Pending blocks that either in the queue or waiting to be queued.
    pub(crate) queued_blocks: QueuedBlocks,
    /// The highest block number that is either in the queue or waiting to be queued.
    pub(crate) best_queued_number: u32,
    /// Orphan blocks
    pub(crate) orphan_blocks_pool: OrphanBlocksPool,
    /// Last time at which the block was received or imported.
    ///
    /// This is updated whenever a block is received from the network or
    /// when the results of processed blocks are notified. It helps track
    /// the most recent activity related to block processing.
    pub(crate) last_progress_time: Instant,
    /// Import queue status.
    pub(crate) queue_status: ImportQueueStatus,
    /// Last time the log of too many blocks in the queue was printed.
    pub(crate) last_overloaded_queue_log_time: Option<Instant>,
    /// Peer store.
    pub(crate) peer_store: Arc<dyn PeerStore>,
}

impl BlockDownloader {
    pub(crate) fn new(
        peer_id: PeerId,
        best_queued_number: u32,
        peer_store: Arc<dyn PeerStore>,
    ) -> Self {
        Self {
            peer_id,
            missing_blocks: Vec::new(),
            requested_blocks: HashSet::new(),
            downloaded_blocks: Vec::new(),
            blocks_in_queue: HashMap::new(),
            queued_blocks: QueuedBlocks::default(),
            best_queued_number,
            orphan_blocks_pool: OrphanBlocksPool::new(),
            last_progress_time: Instant::now(),
            queue_status: ImportQueueStatus::Ready,
            last_overloaded_queue_log_time: None,
            peer_store,
        }
    }

    pub(crate) fn set_missing_blocks(&mut self, new: Vec<BlockHash>) {
        self.missing_blocks = new;
    }

    pub(crate) fn clear_missing_blocks(&mut self) {
        self.missing_blocks.clear();
    }

    /// Prepares and truncates the block data request to avoid overly large requests.
    ///
    /// To minimize potential failures and latency, this function splits large `Inventory` lists
    /// into smaller requests if they exceed a predefined maximum size. If truncation is necessary,
    /// the remaining items are stored in `self.missing_blocks` for future processing.
    ///
    /// # Returns
    ///
    /// A `SyncAction` containing a `SyncRequest::Data` with the truncated list of blocks to
    /// request from the peer.
    pub(crate) fn next_block_download_action(&mut self) -> SyncAction {
        let max_request_size = match self.best_queued_number {
            0..=99_999 => 1024,
            100_000..=199_999 => 512,
            200_000..=299_999 => 128,
            300_000..=399_999 => 64,
            400_000..=499_999 => 32,
            500_000..=599_999 => 16,
            600_000..=699_999 => 8,
            700_000..=799_999 => 4,
            _ => 2,
        };

        let mut blocks_to_download = std::mem::take(&mut self.missing_blocks);

        let new_missing_blocks = if blocks_to_download.len() > max_request_size {
            blocks_to_download.split_off(max_request_size)
        } else {
            vec![]
        };

        self.missing_blocks = new_missing_blocks;

        self.requested_blocks = blocks_to_download.into_iter().collect::<HashSet<_>>();

        tracing::debug!(
            from = ?self.peer_id,
            missing_blocks_count = self.missing_blocks.len(),
            "📦 Downloading {} blocks",
            self.requested_blocks.len(),
        );

        let block_data_request = self
            .requested_blocks
            .clone()
            .into_iter()
            .map(Inventory::Block)
            .collect::<Vec<_>>();

        SyncAction::Request(SyncRequest::Data(block_data_request, self.peer_id))
    }

    /// Determine if the downloader is stalled based on the time elapsed since the last progress
    /// update.
    ///    
    /// The stall detection is influenced by the size of the blockchain and the time since the last
    /// successful block processing. As the chain grows, the following factors contribute to the need
    /// for an extended timeout:
    ///
    /// - **Increased Local Block Execution Time**: Larger chain state lead to longer execution times
    ///   when processing blocks (primarily due to the state root computation).
    /// - **Higher Average Block Size**: As the blockchain grows, the average size of blocks typically
    ///   increases, resulting in longer network response times for block retrieval.
    ///
    /// The timeout values are configurated arbitrarily.
    pub(crate) fn has_stalled(&self) -> Option<PeerId> {
        let stall_timeout = match self.best_queued_number {
            0..300_000 => 60,        // Standard timeout, 1 minute
            300_000..600_000 => 120, // Extended timeout, 2 minutes
            600_000..800_000 => 180, // Extended timeout, 3 minutes
            _ => 300,
        };

        let stalled = self.last_progress_time.elapsed().as_secs() > stall_timeout;

        if stalled {
            self.peer_store.record_failure(self.peer_id);
            Some(self.peer_id)
        } else {
            None
        }
    }

    pub(crate) fn block_exists(&self, block_hash: BlockHash) -> bool {
        self.queued_blocks.block_number(block_hash).is_some()
    }

    pub(crate) fn block_number(&self, block_hash: BlockHash) -> Option<u32> {
        self.queued_blocks.block_number(block_hash)
    }

    pub(crate) fn is_unknown_block(&self, block_hash: BlockHash) -> bool {
        self.queued_blocks.block_number(block_hash).is_none()
            && !self.requested_blocks.contains(&block_hash)
            && !self.orphan_blocks_pool.block_exists(&block_hash)
    }

    pub(crate) fn on_block_response(&mut self, block_hash: BlockHash) -> bool {
        self.last_progress_time = Instant::now();
        self.requested_blocks.remove(&block_hash)
    }

    /// Checks if the import queue is overloaded and updates the internal state.
    pub(crate) fn evaluate_queue_status(&mut self, best_number: u32) -> ImportQueueStatus {
        // Maximum number of pending blocks in the import queue.
        let max_queued_blocks = match best_number {
            0..=100_000 => 8192,
            100_001..=200_000 => 4096,
            200_001..=300_000 => 2048,
            300_001..=600_000 => 1024,
            _ => 512,
        };

        let queued_blocks = self.best_queued_number.saturating_sub(best_number);

        if queued_blocks > max_queued_blocks {
            self.queue_status = ImportQueueStatus::Overloaded;

            if self
                .last_overloaded_queue_log_time
                .map(|last_time| last_time.elapsed() > BUSY_QUEUE_LOG_INTERVAL)
                .unwrap_or(true)
            {
                tracing::debug!(
                    best_number,
                    best_queued_number = self.best_queued_number,
                    "⏸️ Pausing download: too many blocks ({queued_blocks}) in the queue",
                );
                self.last_overloaded_queue_log_time.replace(Instant::now());
            }
        } else {
            self.queue_status = ImportQueueStatus::Ready;
        }

        self.queue_status
    }

    pub(crate) fn reset(&mut self) {
        self.requested_blocks.clear();
        self.downloaded_blocks.clear();
        self.blocks_in_queue.clear();
        self.queued_blocks.clear();
        self.orphan_blocks_pool.clear();
        self.last_progress_time = Instant::now();
        self.queue_status = ImportQueueStatus::Ready;
    }

    pub(crate) fn reset_requested_blocks(&mut self, new: HashSet<BlockHash>) -> HashSet<BlockHash> {
        std::mem::replace(&mut self.requested_blocks, new)
    }

    /// Checks if there are blocks ready to be imported.
    pub(crate) fn has_pending_blocks(&self) -> bool {
        !self.downloaded_blocks.is_empty()
    }

    pub(crate) fn blocks_in_queue_count(&self) -> usize {
        self.blocks_in_queue.len()
    }

    /// Handles blocks that have been processed.
    pub(crate) fn handle_processed_blocks(&mut self, results: ImportManyBlocksResult) {
        self.last_progress_time = Instant::now();
        for (import_result, hash) in &results.results {
            self.blocks_in_queue.remove(hash);
            self.queued_blocks.remove(hash);

            match import_result {
                Ok(_) => {}
                Err(BlockImportError::UnknownParent) => panic!("Unknown parent {hash}"),
                Err(err) => {
                    // TODO: handle error properly
                    panic!("Failed to import block {hash:?}: {err:?}");
                }
            }
        }
    }

    /// Takes downloaded blocks and prepares them for import.
    pub(crate) fn prepare_blocks_for_import(&mut self) -> (Vec<BlockHash>, Vec<BitcoinBlock>) {
        let blocks = std::mem::take(&mut self.downloaded_blocks);

        let mut blocks = blocks
            .into_iter()
            .map(|block| {
                let block_hash = block.block_hash();

                let block_number = self.queued_blocks.block_number(block_hash).expect(
                    "Corrupted state, number for {block_hash} not found in `queued_blocks`",
                );

                self.blocks_in_queue.insert(block_hash, block_number);

                (block_number, block)
            })
            .collect::<Vec<_>>();

        // Ensure the blocks sent to the import queue are ordered.
        blocks.sort_by(|a, b| a.0.cmp(&b.0));

        blocks
            .into_iter()
            .map(|(number, block)| {
                (
                    self.queued_blocks
                        .block_hash(number)
                        .expect("Corrupted state, hash for {number} not found in `queued_blocks`"),
                    block,
                )
            })
            .unzip()
    }

    /// Add the block that is ready to be imported.
    pub(crate) fn add_block(
        &mut self,
        block_number: u32,
        block_hash: BlockHash,
        block: BitcoinBlock,
        from: PeerId,
    ) {
        let mut insert_block = |block_number, block_hash, block| {
            self.downloaded_blocks.push(block);
            self.queued_blocks.insert(block_number, block_hash);
            if block_number > self.best_queued_number {
                self.best_queued_number = block_number;
            }
        };

        insert_block(block_number, block_hash, block);

        let children = self.orphan_blocks_pool.remove_blocks_for_parent(block_hash);

        if !children.is_empty() {
            for (index, child_block) in children.into_iter().enumerate() {
                let hash = child_block.block_hash();
                let number = block_number + index as u32 + 1;
                insert_block(number, hash, child_block);
            }
        }

        self.peer_store.record_block_download(from);
    }

    pub(crate) fn add_orphan_block(&mut self, block_hash: BlockHash, orphan_block: BitcoinBlock) {
        tracing::debug!(
            orphan_blocks_count = self.orphan_blocks_pool.len(),
            "Adding orphan block {block_hash} to the orphan blocks pool",
        );
        self.orphan_blocks_pool.insert_orphan_block(orphan_block);
    }

    pub(crate) fn add_unknown_block(&mut self, block_hash: BlockHash, unknown_block: BitcoinBlock) {
        tracing::debug!(
            orphan_blocks_count = self.orphan_blocks_pool.len(),
            "Adding unknown block {block_hash} to the orphan blocks pool",
        );
        self.orphan_blocks_pool.insert_unknown_block(unknown_block);
    }
}
