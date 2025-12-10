//! Block indexer implementation.

use crate::db::{IndexerDatabase, Result};
use crate::queries::IndexerQuery;
use crate::types::IndexerState;
use bitcoin::{Block as BitcoinBlock, Network};
use futures::StreamExt;
use rayon::prelude::*;
use sc_client_api::{BlockBackend, BlockchainEvents, HeaderBackend};
use sp_runtime::traits::{Block as BlockT, Header, SaturatedConversion};
use std::marker::PhantomData;
use std::path::Path;
use std::sync::Arc;
use subcoin_primitives::BitcoinTransactionAdapter;

/// How often to save progress during historical indexing.
const PROGRESS_SAVE_INTERVAL: u32 = 1000;

/// Number of blocks to batch together during historical indexing.
const BATCH_SIZE: u32 = 100;

/// The main blockchain indexer.
pub struct Indexer<Block, Client, TransactionAdapter> {
    db: IndexerDatabase,
    client: Arc<Client>,
    network: Network,
    _phantom: PhantomData<(Block, TransactionAdapter)>,
}

impl<Block, Client, TransactionAdapter> Indexer<Block, Client, TransactionAdapter>
where
    Block: BlockT,
    Client: BlockchainEvents<Block>
        + HeaderBackend<Block>
        + BlockBackend<Block>
        + Send
        + Sync
        + 'static,
    TransactionAdapter: BitcoinTransactionAdapter<Block> + Send + Sync,
{
    /// Create a new indexer.
    ///
    /// This only initializes the database connection. Historical indexing is deferred
    /// to `run()` to avoid blocking node startup (including the RPC server).
    pub async fn new(db_path: &Path, network: Network, client: Arc<Client>) -> Result<Self> {
        let db = IndexerDatabase::open(db_path, network).await?;

        let indexer = Self {
            db,
            client,
            network,
            _phantom: PhantomData,
        };

        Ok(indexer)
    }

    /// Get a query interface for this indexer.
    pub fn query(&self) -> IndexerQuery {
        IndexerQuery::new(self.db.clone())
    }

    /// Check for indexing gaps and fill them.
    async fn handle_index_gap(&self) -> Result<()> {
        let best_number: u32 = self.client.info().best_number.saturated_into();

        match self.db.load_state().await? {
            Some(IndexerState::HistoricalIndexing {
                target_height,
                current_height,
            }) => {
                // Resume interrupted historical indexing
                tracing::info!(
                    current_height,
                    target_height,
                    "Resuming interrupted transaction indexing"
                );
                self.index_block_range(current_height, target_height)
                    .await?;
            }
            Some(IndexerState::Active { last_indexed }) => {
                // Check if we fell behind
                if last_indexed < best_number {
                    tracing::info!(
                        last_indexed,
                        best_number,
                        "Transaction index behind, catching up"
                    );
                    self.index_block_range(last_indexed + 1, best_number + 1)
                        .await?;
                }
            }
            None => {
                // Fresh start - index all existing blocks
                if best_number > 0 {
                    tracing::info!(
                        best_number,
                        "First run with indexer, indexing all {} existing blocks",
                        best_number + 1
                    );

                    // Save state before starting
                    self.db
                        .save_state(&IndexerState::HistoricalIndexing {
                            target_height: best_number + 1,
                            current_height: 0,
                        })
                        .await?;

                    self.index_block_range(0, best_number + 1).await?;
                } else {
                    // No blocks yet
                    self.db
                        .save_state(&IndexerState::Active { last_indexed: 0 })
                        .await?;
                }
            }
        }

        Ok(())
    }

    /// Index a range of blocks.
    async fn index_block_range(&self, start: u32, end: u32) -> Result<()> {
        let total_blocks = end.saturating_sub(start);
        let start_time = std::time::Instant::now();
        let mut processed = 0u32;

        tracing::info!(
            start,
            end,
            total = total_blocks,
            batch_size = BATCH_SIZE,
            "Starting historical indexing with batched inserts"
        );

        let mut height = start;
        while height < end {
            let batch_end = (height + BATCH_SIZE).min(end);

            // Process a batch of blocks in a single transaction
            self.index_block_batch(height, batch_end).await?;

            let batch_processed = batch_end - height;
            processed += batch_processed;
            height = batch_end;

            // Save progress periodically
            let is_last = height >= end;
            if processed % PROGRESS_SAVE_INTERVAL == 0 || is_last || processed == batch_processed {
                if !is_last {
                    self.db
                        .save_state(&IndexerState::HistoricalIndexing {
                            target_height: end,
                            current_height: height,
                        })
                        .await?;
                }

                // Log progress
                let elapsed = start_time.elapsed().as_secs_f64();
                let blocks_per_sec = if elapsed > 0.0 {
                    processed as f64 / elapsed
                } else {
                    0.0
                };
                let remaining = total_blocks.saturating_sub(processed);
                let eta_secs = if blocks_per_sec > 0.0 {
                    (remaining as f64 / blocks_per_sec) as u64
                } else {
                    0
                };

                tracing::info!(
                    processed,
                    total = total_blocks,
                    percent = format!("{:.1}%", (processed as f64 / total_blocks as f64) * 100.0),
                    blocks_per_sec = format!("{:.0}", blocks_per_sec),
                    eta_secs,
                    "Indexing progress"
                );
            }
        }

        // Transition to active state
        let last_indexed = end.saturating_sub(1);
        self.db
            .save_state(&IndexerState::Active { last_indexed })
            .await?;

        tracing::info!(
            blocks = total_blocks,
            duration_secs = start_time.elapsed().as_secs(),
            "Historical indexing complete"
        );

        Ok(())
    }

    /// Index a batch of blocks in a single database transaction.
    async fn index_block_batch(&self, start: u32, end: u32) -> Result<()> {
        // Read blocks in parallel using rayon
        let heights: Vec<u32> = (start..end).collect();
        let blocks_result: std::result::Result<Vec<_>, _> = heights
            .par_iter()
            .map(|&height| {
                self.get_bitcoin_block_at_height(height)
                    .map(|block| (height, block))
            })
            .collect();

        let mut blocks_data = blocks_result?;

        // Sort by height to ensure correct order for DB insertion
        blocks_data.sort_by_key(|(height, _)| *height);

        // Now index all blocks in a single transaction
        self.db
            .index_blocks_batch(&blocks_data, self.network)
            .await?;

        Ok(())
    }

    /// Index a single block (used during live sync).
    /// Wraps all inserts in a single database transaction for atomicity.
    async fn index_block(&self, block: &BitcoinBlock, height: u32) -> Result<()> {
        self.db.index_block(height, block, self.network).await
    }

    /// Get a Bitcoin block at the given height.
    fn get_bitcoin_block_at_height(&self, height: u32) -> Result<BitcoinBlock> {
        let block_hash = self
            .client
            .hash(height.into())
            .map_err(|e| crate::db::Error::Database(sqlx::Error::Protocol(e.to_string())))?
            .ok_or(crate::db::Error::BlockNotFound(height))?;

        let signed_block = self
            .client
            .block(block_hash)
            .map_err(|e| crate::db::Error::Database(sqlx::Error::Protocol(e.to_string())))?
            .ok_or(crate::db::Error::BlockNotFound(height))?;

        let bitcoin_block =
            subcoin_primitives::convert_to_bitcoin_block::<Block, TransactionAdapter>(
                signed_block.block,
            )
            .map_err(|e| crate::db::Error::Database(sqlx::Error::Protocol(format!("{e:?}"))))?;

        Ok(bitcoin_block)
    }

    /// Run the indexer, processing new blocks as they arrive.
    ///
    /// This first catches up any missing blocks (historical indexing), then
    /// processes new block notifications for live indexing.
    pub async fn run(self) {
        // Loop until fully caught up - this avoids buffering notifications during sync
        loop {
            let best_before: u32 = self.client.info().best_number.saturated_into();

            // Catch up any indexing gap
            if let Err(e) = self.handle_index_gap().await {
                tracing::error!(?e, "Failed to handle indexing gap, indexer will not run");
                return;
            }

            // Check if chain progressed during sync
            let best_after: u32 = self.client.info().best_number.saturated_into();
            if best_after == best_before {
                // No new blocks arrived during sync - we're fully caught up
                break;
            }

            tracing::debug!(
                best_before,
                best_after,
                "Chain progressed during sync, catching up new blocks"
            );
        }

        // Get the height we've indexed up to
        let mut last_indexed = match self.db.load_state().await {
            Ok(Some(crate::types::IndexerState::Active { last_indexed })) => last_indexed,
            _ => 0,
        };

        tracing::info!(
            last_indexed,
            "Historical sync complete, switching to live mode"
        );

        // Now subscribe to stream - we're fully caught up, no buffered notifications
        let mut block_import_stream = self.client.every_import_notification_stream();

        while let Some(notification) = block_import_stream.next().await {
            let block_number: u32 = (*notification.header.number()).saturated_into();

            // Handle reorgs
            if let Some(route) = &notification.tree_route {
                // Revert retracted blocks
                if !route.retracted().is_empty() {
                    let revert_to = route
                        .retracted()
                        .first()
                        .map(|b| {
                            let n: u32 = b.number.saturated_into();
                            n.saturating_sub(1)
                        })
                        .unwrap_or(0);

                    tracing::info!(
                        revert_to,
                        retracted = route.retracted().len(),
                        "Handling reorg, reverting to height {revert_to}"
                    );

                    if let Err(e) = self.db.revert_to_height(revert_to).await {
                        tracing::error!(?e, "Failed to revert blocks during reorg");
                        continue;
                    }
                    last_indexed = revert_to;
                }

                // Index enacted blocks
                for hash_and_number in route.enacted() {
                    let height: u32 = hash_and_number.number.saturated_into();
                    match self.get_bitcoin_block_at_height(height) {
                        Ok(block) => {
                            if let Err(e) = self.index_block(&block, height).await {
                                tracing::error!(?e, height, "Failed to index enacted block");
                            } else {
                                last_indexed = height;
                            }
                        }
                        Err(e) => {
                            tracing::error!(?e, height, "Failed to get enacted block");
                        }
                    }
                }
            } else {
                // Normal block import (no reorg)
                match self.get_bitcoin_block_at_height(block_number) {
                    Ok(block) => {
                        if let Err(e) = self.index_block(&block, block_number).await {
                            tracing::error!(?e, block_number, "Failed to index block");
                            continue;
                        }
                        last_indexed = block_number;
                    }
                    Err(e) => {
                        tracing::error!(?e, block_number, "Failed to get block for indexing");
                        continue;
                    }
                }
            }

            // Update state
            if let Err(e) = self
                .db
                .save_state(&IndexerState::Active { last_indexed })
                .await
            {
                tracing::error!(?e, "Failed to save indexer state");
            }
        }
    }
}
