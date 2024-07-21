mod blocks_first;
mod headers_first;

pub use self::blocks_first::BlocksFirstDownloader;
pub use self::headers_first::HeadersFirstDownloader;

use crate::orphan_blocks_pool::OrphanBlocksPool;
use bitcoin::{Block as BitcoinBlock, BlockHash};
use sc_consensus::BlockImportError;
use sc_consensus_nakamoto::ImportManyBlocksResult;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// Manages queued blocks.
#[derive(Default, Debug, Clone)]
pub(crate) struct QueuedBlocks {
    hash2number: HashMap<BlockHash, u32>,
    number2hash: HashMap<u32, BlockHash>,
}

impl QueuedBlocks {
    fn block_hash(&self, number: u32) -> Option<BlockHash> {
        self.number2hash.get(&number).copied()
    }

    fn block_number(&self, hash: BlockHash) -> Option<u32> {
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

/// Manages the of blocks downloaded from the Bitcoin network.
///
/// This struct keeps track of:
/// - Blocks that have been requested from the network but not yet received.
/// - Blocks that have been downloaded but not yet sent to the import queue.
/// - Blocks that are currently in the import queue awaiting processing.
/// - The highest block number that is either in the queue or pending to be queued.
///
/// [`BlockDownloadManager`] is designed to be used in both Blocks-First and
/// Headers-First sync strategies, providing a common component for managing
/// the state of blocks during the sync process.
#[derive(Debug, Clone)]
pub(crate) struct BlockDownloadManager {
    /// A set of block hashes that have been requested from the network.
    ///
    /// This helps in tracking which blocks are pending download.
    requested_blocks: HashSet<BlockHash>,
    /// A vector of blocks that have been downloaded from the network.
    ///
    /// These blocks are ready to be sent to the import queue.
    downloaded_blocks: Vec<BitcoinBlock>,
    /// A map of block hashes to their respective heights in the import queue.
    /// This helps in tracking which blocks are currently being processed in the import queue.
    blocks_in_queue: HashMap<BlockHash, u32>,
    /// Pending blocks that either in the queue or waiting to be queued.
    queued_blocks: QueuedBlocks,
    /// The highest block number that is either in the queue or waiting to be queued.
    best_queued_number: u32,
    /// Orphan blocks
    orphan_blocks_pool: OrphanBlocksPool,
    /// Last time at which the block was received or imported.
    last_progress_time: Instant,
    /// Whether there are too many blocks in the queue.
    import_queue_is_overloaded: bool,
    /// Last time the log of too many blocks in the queue was printed.
    last_overloaded_queue_log_time: Option<Instant>,
}

impl BlockDownloadManager {
    fn new() -> Self {
        Self {
            requested_blocks: HashSet::new(),
            downloaded_blocks: Vec::new(),
            blocks_in_queue: HashMap::new(),
            queued_blocks: QueuedBlocks::default(),
            best_queued_number: 0u32,
            orphan_blocks_pool: OrphanBlocksPool::new(),
            last_progress_time: Instant::now(),
            import_queue_is_overloaded: false,
            last_overloaded_queue_log_time: None,
        }
    }

    fn is_stalled(&self) -> bool {
        // The downloader is considered as stalled if no progress for some time.
        let stall_timeout = if self.best_queued_number > 300_000 {
            120
        } else {
            60
        };

        self.last_progress_time.elapsed().as_secs() > stall_timeout
    }

    fn block_exists(&self, block_hash: BlockHash) -> bool {
        self.queued_blocks.block_number(block_hash).is_some()
    }

    fn block_number(&self, block_hash: BlockHash) -> Option<u32> {
        self.queued_blocks.block_number(block_hash)
    }

    fn is_unknown_block(&self, block_hash: BlockHash) -> bool {
        self.queued_blocks.block_number(block_hash).is_none()
            && !self.requested_blocks.contains(&block_hash)
            && !self.orphan_blocks_pool.block_exists(&block_hash)
    }

    fn on_block_response(&mut self, block_hash: BlockHash) -> bool {
        self.last_progress_time = Instant::now();
        self.requested_blocks.remove(&block_hash)
    }

    /// Checks if the import queue is overloaded and updates the internal state.
    fn update_and_check_queue_status(&mut self, best_number: u32) -> bool {
        // Maximum number of pending blocks in the import queue.
        let max_queued_blocks = match best_number {
            0..=100_000 => 8192,
            100_001..=200_000 => 4096,
            200_001..=300_000 => 1024,
            _ => 512,
        };

        let import_queue_is_overloaded = self.best_queued_number - best_number > max_queued_blocks;

        if import_queue_is_overloaded {
            const INTERVAL: Duration = Duration::from_secs(5);

            if self
                .last_overloaded_queue_log_time
                .map(|last_time| last_time.elapsed() > INTERVAL)
                .unwrap_or(true)
            {
                tracing::debug!(
                    best_number,
                    best_queued_number = self.best_queued_number,
                    "⏸️ Pausing download: too many blocks in the queue",
                );
                self.last_overloaded_queue_log_time.replace(Instant::now());
            }
        }

        self.import_queue_is_overloaded = import_queue_is_overloaded;

        self.import_queue_is_overloaded
    }

    fn reset(&mut self) {
        self.requested_blocks.clear();
        self.downloaded_blocks.clear();
        self.blocks_in_queue.clear();
        self.queued_blocks.clear();
        self.best_queued_number = 0u32;
        self.orphan_blocks_pool.clear();
        self.last_progress_time = Instant::now();
        self.import_queue_is_overloaded = false;
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

    /// Add the block the queue that is ready to be imported.
    fn add_block(&mut self, block_number: u32, block_hash: BlockHash, block: BitcoinBlock) {
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
    }

    fn add_orphan_block(&mut self, block_hash: BlockHash, orphan_block: BitcoinBlock) {
        tracing::debug!(
            orphan_blocks_count = self.orphan_blocks_pool.len(),
            "Adding orphan block {block_hash} to the orphan blocks pool",
        );
        self.orphan_blocks_pool.insert_orphan_block(orphan_block);
    }

    fn add_unknown_block(&mut self, block_hash: BlockHash, unknown_block: BitcoinBlock) {
        tracing::debug!(
            orphan_blocks_count = self.orphan_blocks_pool.len(),
            "Adding unknown block {block_hash} to the orphan blocks pool",
        );
        self.orphan_blocks_pool.insert_unknown_block(unknown_block);
    }
}
