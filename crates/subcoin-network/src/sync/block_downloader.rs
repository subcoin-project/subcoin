use super::orphan_blocks_pool::OrphanBlocksPool;
use crate::peer_store::PeerStore;
use crate::sync::SyncAction;
use crate::{MemoryConfig, PeerId};
use bitcoin::consensus::Encodable;
use bitcoin::io::{self, Write};
use bitcoin::p2p::message_blockdata::Inventory;
use bitcoin::{Block as BitcoinBlock, BlockHash};
use sc_consensus::BlockImportError;
use sc_consensus_nakamoto::ImportManyBlocksResult;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Interval for logging when the block import queue is too busy.
const BUSY_QUEUE_LOG_INTERVAL: Duration = Duration::from_secs(5);

/// Bitcoin block header size in bytes.
const BITCOIN_BLOCK_HEADER_SIZE: usize = 80;

/// Average transaction size in bytes for fallback memory estimation.
/// Based on empirical data from Bitcoin mainnet where typical transaction sizes
/// range from ~200-600 bytes, with 500 bytes being a reasonable average.
const AVERAGE_TRANSACTION_SIZE_BYTES: usize = 500;

/// Additional memory overhead per block for internal data structures.
const BLOCK_MEMORY_OVERHEAD: usize = 128;

/// A writer that only counts the number of bytes written, without storing them.
/// This is used for efficient size calculation without memory allocation.
struct SizeCounter {
    size: usize,
}

impl SizeCounter {
    fn new() -> Self {
        Self { size: 0 }
    }

    fn size(&self) -> usize {
        self.size
    }
}

impl Write for SizeCounter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.size += buf.len();
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Manages queued blocks and their memory usage.
#[derive(Default, Debug, Clone)]
pub(crate) struct QueuedBlocks {
    hash2number: HashMap<BlockHash, u32>,
    number2hash: HashMap<u32, BlockHash>,
    /// Tracks memory usage for each block hash.
    block_memory: HashMap<BlockHash, usize>,
}

impl QueuedBlocks {
    pub(crate) fn block_hash(&self, number: u32) -> Option<BlockHash> {
        self.number2hash.get(&number).copied()
    }

    pub(crate) fn block_number(&self, hash: BlockHash) -> Option<u32> {
        self.hash2number.get(&hash).copied()
    }

    /// Gets the cached memory usage for a block.
    pub(crate) fn block_memory_usage(&self, hash: BlockHash) -> Option<usize> {
        self.block_memory.get(&hash).copied()
    }

    fn insert(&mut self, number: u32, hash: BlockHash, memory_usage: usize) {
        self.hash2number.insert(hash, number);
        self.number2hash.insert(number, hash);
        self.block_memory.insert(hash, memory_usage);
    }

    fn remove(&mut self, hash: &BlockHash) {
        if let Some(number) = self.hash2number.remove(hash) {
            self.number2hash.remove(&number);
        }
        self.block_memory.remove(hash);
    }

    #[cfg(test)]
    fn clear(&mut self) {
        self.hash2number.clear();
        self.number2hash.clear();
        self.block_memory.clear();
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ImportQueueStatus {
    /// Queue is ready to accept more blocks for import.
    Ready,
    /// The import system is saturated and cannot accept more blocks at the moment.
    ///
    /// This can occur due to either:
    /// - Too many blocks pending in the import queue (block count limit)
    /// - Memory pressure from downloaded blocks exceeding configured limits
    Saturated,
}

impl ImportQueueStatus {
    pub(crate) fn is_saturated(&self) -> bool {
        matches!(self, Self::Saturated)
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
    pub(super) peer_id: PeerId,
    /// Blocks awaiting to be downloaded.
    pub(super) missing_blocks: Vec<BlockHash>,
    /// A set of block hashes that have been requested from the network.
    ///
    /// This helps tracking which blocks are pending download.
    pub(super) requested_blocks: HashSet<BlockHash>,
    /// A vector of blocks that have been downloaded from the network.
    ///
    /// These blocks are ready to be sent to the import queue.
    pub(super) downloaded_blocks: Vec<BitcoinBlock>,
    /// A map of block hashes to their respective heights the import queue.
    /// This helps tracking which blocks are currently being processed in the import queue.
    pub(super) blocks_in_queue: HashMap<BlockHash, u32>,
    /// Pending blocks that either the queue or waiting to be queued.
    pub(super) queued_blocks: QueuedBlocks,
    /// The highest block number that is either the queue or waiting to be queued.
    pub(super) best_queued_number: u32,
    /// Orphan blocks
    pub(super) orphan_blocks_pool: OrphanBlocksPool,
    /// Last time at which the block was received or imported.
    ///
    /// This is updated whenever a block is received from the network or
    /// when the results of processed blocks are notified. It helps track
    /// the most recent activity related to block processing.
    pub(super) last_progress_time: Instant,
    /// Import queue status.
    pub(super) queue_status: ImportQueueStatus,
    /// Last time the log of saturated queue was printed.
    pub(super) last_saturated_queue_log_time: Option<Instant>,
    /// Peer store.
    pub(super) peer_store: Arc<dyn PeerStore>,
    /// Tracks the number of blocks downloaded from the current sync peer.
    pub(super) downloaded_blocks_count: usize,
    /// Memory management configuration.
    pub(super) memory_config: MemoryConfig,
    /// Current memory usage for downloaded blocks in bytes.
    pub(super) downloaded_blocks_memory: usize,
    /// Size of the last requested batch (for pipelining).
    ///
    /// Used to determine when to request the next batch before all blocks
    /// in the current batch are received.
    last_batch_size: usize,
}

impl BlockDownloader {
    pub(crate) fn new(
        peer_id: PeerId,
        best_queued_number: u32,
        peer_store: Arc<dyn PeerStore>,
        memory_config: MemoryConfig,
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
            last_saturated_queue_log_time: None,
            peer_store,
            downloaded_blocks_count: 0,
            memory_config,
            downloaded_blocks_memory: 0,
            last_batch_size: 0,
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

    /// Checks if there are blocks ready to be imported.
    pub(crate) fn has_pending_blocks(&self) -> bool {
        !self.downloaded_blocks.is_empty()
    }

    pub(crate) fn blocks_in_queue_count(&self) -> usize {
        self.blocks_in_queue.len()
    }

    /// Returns the current memory usage of downloaded blocks.
    #[cfg(test)]
    pub(crate) fn downloaded_blocks_memory_usage(&self) -> usize {
        self.downloaded_blocks_memory
    }

    /// Checks if memory usage exceeds the configured limits.
    pub(crate) fn exceeds_memory_limits(&self) -> bool {
        self.downloaded_blocks_memory > self.memory_config.max_downloaded_blocks_memory
            || self.downloaded_blocks.len() > self.memory_config.max_blocks_in_memory
    }

    /// Checks if we should request the next batch of blocks (pipelining).
    ///
    /// Returns `true` when we've received enough blocks from the current batch
    /// to justify requesting the next batch. This reduces idle time waiting
    /// for the last few blocks in a batch.
    ///
    /// Pipelining is triggered when:
    /// - There are more blocks to download (`missing_blocks` not empty)
    /// - We've received at least 50% of the current batch
    /// - We haven't already started pipelining (requested_blocks not empty)
    pub(crate) fn should_pipeline(&self) -> bool {
        // Only pipeline if there are more blocks to download
        if self.missing_blocks.is_empty() {
            return false;
        }

        // Don't pipeline if we've already sent another request (requested_blocks would be full again)
        // This is detected by checking if requested_blocks is less than half the original batch
        if self.last_batch_size == 0 {
            return false;
        }

        let received_count = self
            .last_batch_size
            .saturating_sub(self.requested_blocks.len());
        let pipeline_threshold = self.last_batch_size / 2;

        // Pipeline when we've received at least 50% of the batch
        received_count >= pipeline_threshold && !self.requested_blocks.is_empty()
    }

    pub(super) fn on_block_response(&mut self, block_hash: BlockHash) -> bool {
        self.last_progress_time = Instant::now();
        self.requested_blocks.remove(&block_hash)
    }

    /// Update the last progress time to prevent stall detection during header download.
    ///
    /// This should be called when headers are received to indicate that progress is being made
    /// even though no blocks have been downloaded yet.
    pub(super) fn touch_progress(&mut self) {
        self.last_progress_time = Instant::now();
    }

    pub(super) fn set_missing_blocks(&mut self, new: Vec<BlockHash>) {
        self.missing_blocks = new;
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

    /// Evaluates the import queue status and updates the internal state.
    ///
    /// The queue is considered saturated if either:
    /// - Too many blocks are pending in the import queue (exceeds block count limits)
    /// - Memory pressure from downloaded blocks exceeds configured limits
    pub(super) fn evaluate_queue_status(&mut self, best_number: u32) -> ImportQueueStatus {
        // Maximum number of pending blocks in the import queue.
        let max_queued_blocks = match best_number {
            0..=100_000 => 8192,
            100_001..=200_000 => 4096,
            200_001..=300_000 => 2048,
            300_001..=600_000 => 1024,
            _ => 512,
        };

        let queued_blocks = self.best_queued_number.saturating_sub(best_number);
        let exceeds_memory_limits = self.exceeds_memory_limits();

        if queued_blocks > max_queued_blocks || exceeds_memory_limits {
            self.queue_status = ImportQueueStatus::Saturated;

            if self
                .last_saturated_queue_log_time
                .is_none_or(|last_time| last_time.elapsed() > BUSY_QUEUE_LOG_INTERVAL)
            {
                if exceeds_memory_limits {
                    let memory_mb = self.downloaded_blocks_memory / (1024 * 1024);
                    let max_memory_mb =
                        self.memory_config.max_downloaded_blocks_memory / (1024 * 1024);
                    tracing::debug!(
                        best_number,
                        memory_usage_mb = memory_mb,
                        max_memory_mb = max_memory_mb,
                        blocks_in_memory = self.downloaded_blocks.len(),
                        max_blocks = self.memory_config.max_blocks_in_memory,
                        "⏸️ Pausing download: memory pressure detected",
                    );
                } else {
                    tracing::debug!(
                        best_number,
                        best_queued_number = self.best_queued_number,
                        "⏸️ Pausing download: too many blocks ({queued_blocks}) in the queue",
                    );
                }
                self.last_saturated_queue_log_time.replace(Instant::now());
            }
        } else {
            self.queue_status = ImportQueueStatus::Ready;
        }

        self.queue_status
    }

    /// Prepares the next block data request, ensuring the request size aligns with the current
    /// blockchain height to avoid overly large downloads and improve latency.
    ///
    /// This function selects a batch of blocks from `self.missing_blocks` based on the
    /// `self.best_queued_number`. As the chain grows, the maximum request size decreases to
    /// reduce the burden on peers. If the number of blocks exceeds the `max_request_size`,
    /// the function truncates the list to the maximum allowed, storing any remaining blocks
    /// back in `self.missing_blocks` for future requests.
    ///
    /// The batch size is now adaptive based on memory pressure and configuration.
    ///
    /// # Returns
    ///
    /// A `SyncAction::Request` containing a `SyncRequest::GetData` with the list of blocks to
    /// request from the peer.
    pub(super) fn schedule_next_download_batch(&mut self) -> SyncAction {
        let base_max_request_size = match self.best_queued_number {
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

        // Apply adaptive batch sizing based on memory pressure
        let max_request_size = if self.memory_config.enable_adaptive_batch_sizing {
            let memory_usage_ratio = self.downloaded_blocks_memory as f64
                / self.memory_config.max_downloaded_blocks_memory as f64;

            let blocks_ratio = self.downloaded_blocks.len() as f64
                / self.memory_config.max_blocks_in_memory as f64;

            let pressure_ratio = memory_usage_ratio.max(blocks_ratio);

            if pressure_ratio > 0.8 {
                // High memory pressure: reduce batch size significantly
                (base_max_request_size / 4).max(1)
            } else if pressure_ratio > 0.6 {
                // Medium memory pressure: reduce batch size moderately
                (base_max_request_size / 2).max(1)
            } else {
                // Low memory pressure: use base batch size
                base_max_request_size
            }
        } else {
            base_max_request_size
        };

        let mut blocks_to_download = std::mem::take(&mut self.missing_blocks);

        let new_missing_blocks = if blocks_to_download.len() > max_request_size {
            blocks_to_download.split_off(max_request_size)
        } else {
            vec![]
        };

        self.missing_blocks = new_missing_blocks;

        // Track batch size for pipelining
        let batch_size = blocks_to_download.len();
        self.last_batch_size = batch_size;

        // Extend requested_blocks instead of replacing (for pipelining support)
        self.requested_blocks
            .extend(blocks_to_download.iter().copied());

        tracing::debug!(
            from = ?self.peer_id,
            pending_blocks_to_download = self.missing_blocks.len(),
            "📦 Downloading {batch_size} blocks",
        );

        let block_data_request = blocks_to_download
            .into_iter()
            .map(Inventory::Block)
            .collect::<Vec<_>>();

        SyncAction::get_data(block_data_request, self.peer_id)
    }

    /// Restarts the block downloader with a new peer.
    ///
    /// This preserves already-downloaded blocks to avoid wasting bandwidth.
    /// Only clears pending requests that won't be fulfilled by the new peer.
    pub(super) fn restart(&mut self, new_peer: PeerId) {
        let preserved_blocks = self.downloaded_blocks.len();
        let preserved_memory = self.downloaded_blocks_memory;

        self.peer_id = new_peer;
        self.downloaded_blocks_count = 0;
        // Clear only pending requests - these won't be fulfilled by the new peer
        self.requested_blocks.clear();
        // Preserve downloaded_blocks - they're still valid and ready for import
        // Preserve blocks_in_queue - they're being processed
        // Preserve queued_blocks - they track block numbers for downloaded blocks
        self.orphan_blocks_pool.clear();
        self.last_progress_time = Instant::now();
        self.queue_status = ImportQueueStatus::Ready;
        // Don't reset downloaded_blocks_memory - we preserved the blocks

        if preserved_blocks > 0 {
            tracing::debug!(
                preserved_blocks,
                preserved_memory_kb = preserved_memory / 1024,
                "Preserved downloaded blocks during peer switch"
            );
        }
    }

    /// Handles blocks that have been processed.
    pub(crate) fn handle_processed_blocks(&mut self, results: ImportManyBlocksResult) {
        self.last_progress_time = Instant::now();
        for (import_result, hash) in &results.results {
            let block_number = self.blocks_in_queue.remove(hash);
            self.queued_blocks.remove(hash);

            match import_result {
                Ok(_) => {}
                Err(BlockImportError::UnknownParent) => panic!("Unknown parent {hash}"),
                Err(err) => {
                    // TODO: handle error properly
                    if let Some(number) = block_number {
                        panic!("Failed to import block #{number:?},{hash:?}: {err:?}");
                    } else {
                        panic!("Failed to import block #{hash:?}: {err:?}");
                    }
                }
            }
        }
    }

    /// Takes downloaded blocks and prepares them for import.
    pub(crate) fn prepare_blocks_for_import(
        &mut self,
        max_block_number: Option<u32>,
    ) -> (Vec<BlockHash>, Vec<BitcoinBlock>) {
        let mut blocks = Vec::new();
        let mut total_memory_freed = 0;

        // Process blocks and track memory being freed
        for block in self.downloaded_blocks.drain(..) {
            let block_hash = block.block_hash();

            let block_number = self
                .queued_blocks
                .block_number(block_hash)
                .unwrap_or_else(|| {
                    panic!("Corrupted state, number for {block_hash} not found in `queued_blocks`")
                });

            // Get cached memory usage to avoid recalculation
            let block_memory = self
                .queued_blocks
                .block_memory_usage(block_hash)
                .unwrap_or_else(|| calculate_block_memory_usage(&block));
            total_memory_freed += block_memory;

            if max_block_number.is_some_and(|target_block| block_number > target_block) {
                // Block is filtered out - don't add to import queue
            } else {
                self.blocks_in_queue.insert(block_hash, block_number);
                blocks.push((block_number, block));
            }
        }

        // Update memory tracking - these blocks are no longer in downloaded_blocks
        self.downloaded_blocks_memory = self
            .downloaded_blocks_memory
            .saturating_sub(total_memory_freed);

        // Ensure the blocks sent to the import queue are ordered.
        blocks.sort_unstable_by_key(|(number, _)| *number);

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
    pub(super) fn add_block(
        &mut self,
        block_number: u32,
        block_hash: BlockHash,
        block: BitcoinBlock,
        from: PeerId,
    ) {
        let mut insert_block = |block_number, block_hash, block: BitcoinBlock| {
            // Track memory usage
            let block_memory = calculate_block_memory_usage(&block);
            self.downloaded_blocks_memory += block_memory;

            self.downloaded_blocks.push(block);
            self.queued_blocks
                .insert(block_number, block_hash, block_memory);
            if block_number > self.best_queued_number {
                self.best_queued_number = block_number;
            }
        };

        insert_block(block_number, block_hash, block);

        let children = self.orphan_blocks_pool.remove_blocks_for_parent(block_hash);

        if !children.is_empty() {
            tracing::trace!(blocks = ?children, "Connected orphan blocks");
            for (index, child_block) in children.into_iter().enumerate() {
                let hash = child_block.block_hash();
                let number = block_number + index as u32 + 1;
                insert_block(number, hash, child_block);
            }
        }

        self.downloaded_blocks_count += 1;
        self.peer_store.record_block_download(from);
    }

    pub(super) fn add_orphan_block(&mut self, block_hash: BlockHash, orphan_block: BitcoinBlock) {
        self.orphan_blocks_pool.insert_orphan_block(orphan_block);
        tracing::debug!(
            orphan_blocks_count = self.orphan_blocks_pool.len(),
            "Added orphan block {block_hash} to orphan blocks pool",
        );
    }

    pub(super) fn add_unknown_block(&mut self, block_hash: BlockHash, unknown_block: BitcoinBlock) {
        self.orphan_blocks_pool.insert_unknown_block(unknown_block);
        tracing::debug!(
            orphan_blocks_count = self.orphan_blocks_pool.len(),
            "Added unknown block {block_hash} to orphan blocks pool",
        );
    }
}

/// Calculates the approximate memory usage of a block.
fn calculate_block_memory_usage(block: &BitcoinBlock) -> usize {
    // Calculate size more efficiently by encoding header and transactions separately
    let header_size = {
        let mut size_counter = SizeCounter::new();
        if block.header.consensus_encode(&mut size_counter).is_ok() {
            size_counter.size()
        } else {
            BITCOIN_BLOCK_HEADER_SIZE
        }
    };

    let txs_size: usize = block
        .txdata
        .iter()
        .map(|tx| {
            let mut size_counter = SizeCounter::new();
            if tx.consensus_encode(&mut size_counter).is_ok() {
                size_counter.size()
            } else {
                AVERAGE_TRANSACTION_SIZE_BYTES
            }
        })
        .sum();

    // Add some overhead for internal data structures
    header_size + txs_size + BLOCK_MEMORY_OVERHEAD
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peer_store::NoPeerStore;
    use std::sync::Arc;

    fn create_test_memory_config() -> MemoryConfig {
        MemoryConfig {
            max_downloaded_blocks_memory: 1024 * 1024, // 1 MB for testing
            max_blocks_in_memory: 5,                   // Small number for easy testing
            max_orphan_blocks_memory: 512 * 1024,      // 512 KB
            enable_adaptive_batch_sizing: true,
        }
    }

    fn create_test_peer_store() -> Arc<dyn crate::peer_store::PeerStore> {
        Arc::new(NoPeerStore)
    }

    #[test]
    fn memory_config_defaults_are_reasonable() {
        let config = MemoryConfig::default();

        assert_eq!(config.max_downloaded_blocks_memory, 256 * 1024 * 1024); // 256 MB
        assert_eq!(config.max_blocks_in_memory, 1000);
        assert_eq!(config.max_orphan_blocks_memory, 64 * 1024 * 1024); // 64 MB
        assert!(config.enable_adaptive_batch_sizing);
    }

    #[test]
    fn block_downloader_memory_tracking() {
        let peer_id: PeerId = "127.0.0.1:8333".parse().unwrap();
        let memory_config = create_test_memory_config();
        let peer_store = create_test_peer_store();

        let mut downloader = BlockDownloader::new(peer_id, 0, peer_store, memory_config.clone());

        // Initially, memory usage should be zero
        assert_eq!(downloader.downloaded_blocks_memory_usage(), 0);
        assert!(!downloader.exceeds_memory_limits());

        // Add a block and verify memory tracking
        let test_blocks = subcoin_test_service::block_data();
        let block = test_blocks[1].clone();
        let block_hash = block.block_hash();

        downloader.add_block(1, block_hash, block.clone(), peer_id);

        // Memory usage should now be greater than zero
        assert!(downloader.downloaded_blocks_memory_usage() > 0);

        // Verify memory usage is tracked in queued_blocks
        let expected_memory = calculate_block_memory_usage(&block);
        assert_eq!(
            downloader.queued_blocks.block_memory_usage(block_hash),
            Some(expected_memory)
        );

        // Verify memory constraint checking
        let initial_memory = downloader.downloaded_blocks_memory_usage();
        assert!(initial_memory < memory_config.max_downloaded_blocks_memory);
        assert!(!downloader.exceeds_memory_limits());
    }

    #[test]
    fn memory_constraint_detection() {
        let peer_id: PeerId = "127.0.0.1:8333".parse().unwrap();
        let mut memory_config = create_test_memory_config();
        memory_config.max_downloaded_blocks_memory = 100; // Very small limit
        memory_config.max_blocks_in_memory = 1; // Only 1 block allowed

        let peer_store = create_test_peer_store();
        let mut downloader = BlockDownloader::new(peer_id, 0, peer_store, memory_config.clone());

        let test_blocks = subcoin_test_service::block_data();
        let block = test_blocks[1].clone();
        let block_hash = block.block_hash();

        // Add one block
        downloader.add_block(1, block_hash, block, peer_id);

        // Should be memory constrained due to block count limit
        assert!(downloader.exceeds_memory_limits());

        // Test with memory size limit
        let mut memory_config2 = create_test_memory_config();
        memory_config2.max_blocks_in_memory = 1000; // High block limit
        memory_config2.max_downloaded_blocks_memory = 1; // Very small memory limit

        let mut downloader2 =
            BlockDownloader::new(peer_id, 0, create_test_peer_store(), memory_config2);
        downloader2.add_block(1, block_hash, test_blocks[1].clone(), peer_id);

        // Should be memory constrained due to memory size limit
        assert!(downloader2.exceeds_memory_limits());
    }

    #[test]
    fn queue_status_evaluation_with_memory_pressure() {
        let peer_id: PeerId = "127.0.0.1:8333".parse().unwrap();
        let mut memory_config = create_test_memory_config();
        memory_config.max_downloaded_blocks_memory = 100; // Small limit to trigger pressure

        let peer_store = create_test_peer_store();
        let mut downloader = BlockDownloader::new(peer_id, 0, peer_store, memory_config.clone());

        // Initially should be ready
        let status = downloader.evaluate_queue_status(0);
        assert!(status.is_ready());

        // Add blocks to trigger memory pressure
        let test_blocks = subcoin_test_service::block_data();
        downloader.add_block(
            1,
            test_blocks[1].block_hash(),
            test_blocks[1].clone(),
            peer_id,
        );

        // Should now be saturated due to memory pressure
        let status = downloader.evaluate_queue_status(0);
        assert!(status.is_saturated());
    }

    #[test]
    fn adaptive_batch_sizing_under_memory_pressure() {
        let peer_id: PeerId = "127.0.0.1:8333".parse().unwrap();
        let memory_config = create_test_memory_config();
        let peer_store = create_test_peer_store();

        // Set best_queued_number to a high value to get a smaller base batch size
        let mut downloader =
            BlockDownloader::new(peer_id, 800_000, peer_store, memory_config.clone());

        // Set up missing blocks for download - base batch size for 800k+ blocks is 2
        let test_blocks = subcoin_test_service::block_data();
        let missing_blocks: Vec<BlockHash> = test_blocks
            .iter()
            .cycle() // Repeat the blocks to create more entries
            .take(10) // Take 10 blocks total (more than base batch size of 2)
            .map(|block| block.block_hash())
            .collect();

        downloader.set_missing_blocks(missing_blocks.clone());

        // Simulate high memory usage by directly setting memory usage and block count
        downloader.downloaded_blocks_memory =
            (memory_config.max_downloaded_blocks_memory as f64 * 0.9) as usize;
        // Also add blocks to memory to increase pressure (reach our test limit of 5)
        for _i in 1..=4 {
            downloader.downloaded_blocks.push(test_blocks[1].clone());
        }

        // Schedule download batch - should be reduced due to memory pressure
        let sync_action = downloader.schedule_next_download_batch();

        if let crate::sync::SyncAction::Request(crate::sync::SyncRequest::Data(inv, _)) =
            sync_action
        {
            // Under high memory pressure, batch size should be 1 (base size 2 / 4 = 0.5, max(1) = 1)
            println!(
                "Requested {} blocks out of {} missing blocks",
                inv.len(),
                missing_blocks.len()
            );
            assert_eq!(
                inv.len(),
                1,
                "Under high memory pressure, batch size should be 1"
            );
            assert!(
                inv.len() < missing_blocks.len(),
                "Batch size should be reduced under memory pressure"
            );
        } else {
            panic!("Expected SyncAction::Request with Data");
        }
    }

    #[test]
    fn adaptive_batch_sizing_disabled() {
        let peer_id: PeerId = "127.0.0.1:8333".parse().unwrap();
        let mut memory_config = create_test_memory_config();
        memory_config.enable_adaptive_batch_sizing = false; // Disable adaptive sizing

        let peer_store = create_test_peer_store();
        let mut downloader = BlockDownloader::new(peer_id, 0, peer_store, memory_config.clone());

        // Set up missing blocks
        let test_blocks = subcoin_test_service::block_data();
        let missing_blocks: Vec<BlockHash> = test_blocks[1..4] // Use fewer blocks
            .iter()
            .map(|block| block.block_hash())
            .collect();

        downloader.set_missing_blocks(missing_blocks.clone());

        // Set high memory usage
        downloader.downloaded_blocks_memory =
            (memory_config.max_downloaded_blocks_memory as f64 * 0.9) as usize;

        let sync_action = downloader.schedule_next_download_batch();

        if let crate::sync::SyncAction::Request(crate::sync::SyncRequest::Data(inv, _)) =
            sync_action
        {
            // With adaptive sizing disabled, should request all blocks regardless of memory pressure
            assert_eq!(inv.len(), missing_blocks.len());
        } else {
            panic!("Expected SyncAction::Request with Data");
        }
    }

    #[test]
    fn memory_cleanup_on_block_processing() {
        let peer_id: PeerId = "127.0.0.1:8333".parse().unwrap();
        let memory_config = create_test_memory_config();
        let peer_store = create_test_peer_store();

        let mut downloader = BlockDownloader::new(peer_id, 0, peer_store, memory_config);

        // Add a block
        let test_blocks = subcoin_test_service::block_data();
        let block = test_blocks[1].clone();
        let block_hash = block.block_hash();

        downloader.add_block(1, block_hash, block, peer_id);

        let initial_memory = downloader.downloaded_blocks_memory_usage();
        assert!(initial_memory > 0);

        // Prepare blocks for import (this should reduce memory usage)
        let (hashes, blocks) = downloader.prepare_blocks_for_import(None);

        assert_eq!(hashes.len(), 1);
        assert_eq!(blocks.len(), 1);

        // Memory usage should be reduced (or zero if all blocks were processed)
        let final_memory = downloader.downloaded_blocks_memory_usage();
        assert!(final_memory < initial_memory);
    }

    #[test]
    fn memory_usage_calculation_accuracy() {
        let peer_id: PeerId = "127.0.0.1:8333".parse().unwrap();
        let memory_config = create_test_memory_config();
        let peer_store = create_test_peer_store();

        let mut downloader = BlockDownloader::new(peer_id, 0, peer_store, memory_config);

        let test_blocks = subcoin_test_service::block_data();

        // Add multiple blocks and verify memory increases
        let mut previous_memory = 0;

        for (i, block) in test_blocks.iter().take(3).enumerate() {
            let block_number = i as u32 + 1;
            let block_hash = block.block_hash();

            downloader.add_block(block_number, block_hash, block.clone(), peer_id);

            let current_memory = downloader.downloaded_blocks_memory_usage();
            assert!(current_memory > previous_memory);
            previous_memory = current_memory;
        }

        // Verify that memory tracking is reasonable (not zero, not extremely large)
        assert!(previous_memory > 1000); // Should be at least 1KB for real blocks
        assert!(previous_memory < 10 * 1024 * 1024); // Should be less than 10MB for test blocks
    }

    #[test]
    fn memory_pressure_affects_queue_status_logging() {
        let peer_id: PeerId = "127.0.0.1:8333".parse().unwrap();
        let mut memory_config = create_test_memory_config();
        memory_config.max_downloaded_blocks_memory = 1; // Extremely small to trigger pressure immediately

        let peer_store = create_test_peer_store();
        let mut downloader = BlockDownloader::new(peer_id, 0, peer_store, memory_config);

        // Add a block to trigger memory pressure
        let test_blocks = subcoin_test_service::block_data();
        downloader.add_block(
            1,
            test_blocks[1].block_hash(),
            test_blocks[1].clone(),
            peer_id,
        );

        // Evaluate queue status multiple times
        let status1 = downloader.evaluate_queue_status(0);
        assert!(status1.is_saturated());

        // Log time should be set after first evaluation
        assert!(downloader.last_saturated_queue_log_time.is_some());

        // Second evaluation should not reset log time immediately
        let status2 = downloader.evaluate_queue_status(0);
        assert!(status2.is_saturated());
    }

    #[test]
    fn queued_blocks_tracks_memory_usage() {
        let mut queued_blocks = QueuedBlocks::default();
        let test_blocks = subcoin_test_service::block_data();
        let test_block = &test_blocks[1];
        let block_hash = test_block.block_hash();

        // Calculate memory usage
        let memory_usage = calculate_block_memory_usage(test_block);
        assert!(memory_usage > 0);

        // Insert block with memory tracking
        queued_blocks.insert(1, block_hash, memory_usage);

        // Verify memory usage is tracked
        assert_eq!(
            queued_blocks.block_memory_usage(block_hash),
            Some(memory_usage)
        );
        assert_eq!(queued_blocks.block_number(block_hash), Some(1));
        assert_eq!(queued_blocks.block_hash(1), Some(block_hash));

        // Test removal clears memory tracking
        queued_blocks.remove(&block_hash);
        assert_eq!(queued_blocks.block_memory_usage(block_hash), None);
        assert_eq!(queued_blocks.block_number(block_hash), None);

        // Test clear removes all memory tracking
        queued_blocks.insert(1, block_hash, memory_usage);
        queued_blocks.insert(2, test_blocks[2].block_hash(), memory_usage + 100);
        queued_blocks.clear();
        assert_eq!(queued_blocks.block_memory_usage(block_hash), None);
        assert_eq!(
            queued_blocks.block_memory_usage(test_blocks[2].block_hash()),
            None
        );
    }

    #[test]
    fn memory_cleanup_uses_cached_values() {
        let peer_id: PeerId = "127.0.0.1:8333".parse().unwrap();
        let memory_config = create_test_memory_config();
        let peer_store = create_test_peer_store();

        let mut downloader = BlockDownloader::new(peer_id, 0, peer_store, memory_config);

        // Add a block
        let test_blocks = subcoin_test_service::block_data();
        let block = test_blocks[1].clone();
        let block_hash = block.block_hash();

        downloader.add_block(1, block_hash, block.clone(), peer_id);

        let initial_memory = downloader.downloaded_blocks_memory_usage();
        assert!(initial_memory > 0);

        // Verify the memory is cached in queued_blocks
        let cached_memory = downloader.queued_blocks.block_memory_usage(block_hash);
        assert!(cached_memory.is_some());
        assert_eq!(cached_memory.unwrap(), calculate_block_memory_usage(&block));

        // Prepare blocks for import (this should use cached memory values)
        let (hashes, blocks) = downloader.prepare_blocks_for_import(None);

        assert_eq!(hashes.len(), 1);
        assert_eq!(blocks.len(), 1);

        // Memory usage should be reduced
        let final_memory = downloader.downloaded_blocks_memory_usage();
        assert!(final_memory < initial_memory);

        // Block should still be tracked in queued_blocks until fully processed
        assert!(
            downloader
                .queued_blocks
                .block_memory_usage(block_hash)
                .is_some()
        );

        // Simulate processing completion
        use sc_consensus::BlockImportStatus;
        use sc_consensus_nakamoto::ImportManyBlocksResult;
        let results = ImportManyBlocksResult {
            imported: 1,
            block_count: 1,
            results: vec![(
                Ok(BlockImportStatus::ImportedUnknown(
                    1,
                    Default::default(),
                    None,
                )),
                block_hash,
            )],
        };
        downloader.handle_processed_blocks(results);

        // Now the block should no longer be tracked in queued_blocks memory
        assert_eq!(
            downloader.queued_blocks.block_memory_usage(block_hash),
            None
        );
    }
}
