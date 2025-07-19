//! This module defines the [`BlockImportQueue`] struct, which separates the block download
//! and import processes. The queue operates in a distinct blocking task, receiving blocks
//! downloaded from the Bitcoin P2P network by subcoin-network and importing them into
//! the database. Import results are then communicated back.

use crate::block_import::{BitcoinBlockImport, ImportStatus};
use bitcoin::{Block as BitcoinBlock, BlockHash};
use futures::StreamExt;
use futures::prelude::*;
use futures::task::{Context, Poll};
use sc_consensus::{BlockImportError, BlockImportParams};
use sc_utils::mpsc::{TracingUnboundedReceiver, TracingUnboundedSender, tracing_unbounded};
use sp_consensus::BlockOrigin;
use sp_core::traits::SpawnEssentialNamed;
use sp_runtime::traits::Block as BlockT;
use std::pin::Pin;

/// Represents a batch of Bitcoin blocks that are to be imported.
#[derive(Debug, Clone)]
pub struct ImportBlocks {
    /// The source from which the blocks were obtained.
    pub origin: BlockOrigin,
    /// A vector containing the Bitcoin blocks to be imported.
    pub blocks: Vec<BitcoinBlock>,
}

/// Subcoin import queue for processing Bitcoin blocks.
#[derive(Debug)]
pub struct BlockImportQueue {
    block_import_sender: TracingUnboundedSender<ImportBlocks>,
    import_result_receiver: TracingUnboundedReceiver<ImportManyBlocksResult>,
}

impl BlockImportQueue {
    /// Sends a batch of blocks to the worker of import queue for processing.
    pub fn import_blocks(&self, incoming_blocks: ImportBlocks) {
        let _ = self.block_import_sender.unbounded_send(incoming_blocks);
    }

    /// Retrieves the results of the block import operations.
    ///
    /// This asynchronous function waits for and returns the results of the block import process.
    /// It consumes the next available result from the import queue.
    pub async fn block_import_results(&mut self) -> ImportManyBlocksResult {
        self.import_result_receiver.select_next_some().await
    }
}

/// Creates a new import queue.
pub fn bitcoin_import_queue<BI>(
    spawner: &impl SpawnEssentialNamed,
    mut block_import: BI,
) -> BlockImportQueue
where
    BI: BitcoinBlockImport,
{
    let (import_result_sender, import_result_receiver) =
        tracing_unbounded("mpsc_subcoin_import_queue_result", 100_000);

    let (block_import_sender, block_import_receiver) =
        tracing_unbounded("mpsc_subcoin_import_queue_worker_blocks", 100_000);

    let future = async move {
        let block_import_process = block_import_process(
            &mut block_import,
            import_result_sender.clone(),
            block_import_receiver,
        );
        futures::pin_mut!(block_import_process);

        loop {
            // If the results sender is closed, that means that the import queue is shutting
            // down and we should end this future.
            if import_result_sender.is_closed() {
                tracing::debug!("Stopping block import because result channel was closed!");
                return;
            }

            if let Poll::Ready(()) = futures::poll!(&mut block_import_process) {
                return;
            }

            // All futures that we polled are now pending.
            futures::pending!()
        }
    };

    spawner.spawn_essential_blocking(
        "bitcoin-block-import-worker",
        Some("bitcoin-block-import"),
        future.boxed(),
    );

    BlockImportQueue {
        block_import_sender,
        import_result_receiver,
    }
}

/// A dummy verifier that verifies nothing against the block.
pub struct VerifyNothing;

#[async_trait::async_trait]
impl<Block: BlockT> sc_consensus::Verifier<Block> for VerifyNothing {
    async fn verify(
        &self,
        block: BlockImportParams<Block>,
    ) -> Result<BlockImportParams<Block>, String> {
        Ok(BlockImportParams::new(block.origin, block.header))
    }
}

/// The process of importing blocks.
///
/// This polls the `block_import_receiver` for new blocks to import and than awaits on
/// importing these blocks. After each block is imported, this async function yields once
/// to give other futures the possibility to be run.
///
/// Returns when `block_import` ended.
async fn block_import_process(
    block_import: &mut dyn BitcoinBlockImport,
    result_sender: TracingUnboundedSender<ImportManyBlocksResult>,
    mut block_import_receiver: TracingUnboundedReceiver<ImportBlocks>,
) {
    loop {
        let Some(ImportBlocks { origin, blocks }) = block_import_receiver.next().await else {
            tracing::debug!("Stopping block import because the import channel was closed!",);
            return;
        };

        let res = import_many_blocks(block_import, origin, blocks).await;

        let _ = result_sender.unbounded_send(res);
    }
}

type BlockImportStatus = sc_consensus::BlockImportStatus<u32>;

/// Result of `import_many_blocks`.
#[derive(Debug)]
pub struct ImportManyBlocksResult {
    /// The number of blocks imported successfully.
    pub imported: usize,
    /// The total number of blocks processed.
    pub block_count: usize,
    /// The import results for each block.
    pub results: Vec<(Result<BlockImportStatus, BlockImportError>, BlockHash)>,
}

/// Import several blocks at once, returning import result for each block.
///
/// This will yield after each imported block once, to ensure that other futures can
/// be called as well.
async fn import_many_blocks(
    import_handle: &mut dyn BitcoinBlockImport,
    origin: BlockOrigin,
    blocks: Vec<BitcoinBlock>,
) -> ImportManyBlocksResult {
    tracing::trace!("[import_many_blocks] importing {} blocks", blocks.len());
    let count = blocks.len();

    let mut imported = 0;
    let mut results = vec![];
    let mut has_error = false;

    // Blocks in the response/drain should be in ascending order.
    for block in blocks {
        let block_hash = block.block_hash();

        let block_import_result = if has_error {
            Err(BlockImportError::Cancelled)
        } else {
            // The actual import.
            let import_result = import_handle.import_block(block, origin).await;

            match import_result {
                Ok(ImportStatus::AlreadyInChain(number)) => {
                    tracing::trace!("Block already in chain: #{number},{block_hash:?}");
                    Ok(BlockImportStatus::ImportedKnown(number, None))
                }
                Ok(ImportStatus::Imported {
                    block_number,
                    block_hash: _,
                    aux,
                }) => Ok(BlockImportStatus::ImportedUnknown(block_number, aux, None)),
                Ok(ImportStatus::MissingState) => {
                    tracing::debug!("Parent state is missing for block: {block_hash:?}",);
                    Err(BlockImportError::MissingState)
                }
                Ok(ImportStatus::UnknownParent) => {
                    tracing::debug!("Block {block_hash:?} has unknown parent");
                    Err(BlockImportError::UnknownParent)
                }
                Ok(ImportStatus::KnownBad) => {
                    tracing::debug!("Peer gave us a bad block: {block_hash:?}");
                    Err(BlockImportError::BadBlock(None))
                }
                Err(err) => {
                    tracing::error!(?err, "Error importing block: {block_hash:?}");
                    Err(BlockImportError::Other(err))
                }
            }
        };

        if let Ok(block_import_status) = &block_import_result {
            let block_number = block_import_status.number();
            tracing::debug!("Block imported successfully #{block_number:?},{block_hash}");
            imported += 1;
        } else {
            has_error = true;
        }

        results.push((block_import_result, block_hash));

        Yield::new().await
    }

    // No block left to import, success!
    ImportManyBlocksResult {
        block_count: count,
        imported,
        results,
    }
}

/// A future that will always `yield` on the first call of `poll` but schedules the
/// current task for re-execution.
///
/// This is done by getting the waker and calling `wake_by_ref` followed by returning
/// `Pending`. The next time the `poll` is called, it will return `Ready`.
struct Yield(bool);

impl Yield {
    fn new() -> Self {
        Self(false)
    }
}

impl Future for Yield {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if !self.0 {
            self.0 = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    }
}
