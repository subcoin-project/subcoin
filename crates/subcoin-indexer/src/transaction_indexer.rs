use bitcoin::Txid;
use bitcoin::hashes::Hash;
use codec::{Decode, Encode};
use futures::StreamExt;
use sc_client_api::backend::AuxStore;
use sc_client_api::{BlockBackend, BlockchainEvents, HeaderBackend, StorageProvider};
use sc_service::SpawnTaskHandle;
use sp_runtime::generic::SignedBlock;
use sp_runtime::traits::{Block as BlockT, Header, SaturatedConversion};
use std::marker::PhantomData;
use std::sync::Arc;
use subcoin_primitives::{BitcoinTransactionAdapter, TransactionIndex, TxPosition};

type IndexRange = std::ops::Range<u32>;

/// Represents actions applied to a block during transaction indexing.
#[derive(Debug, Clone, Copy)]
enum IndexAction {
    Apply,
    Revert,
}

const INDEXER_STATE_KEY: &[u8] = b"tx_indexer_state";

/// Tracks the transaction indexer's state for crash recovery and gap detection.
///
/// This state machine ensures that:
/// 1. Fresh starts on existing chains index all historical blocks
/// 2. Interrupted historical indexing can be resumed
/// 3. Normal operation tracks the last indexed block
#[derive(Debug, Clone, Encode, Decode)]
pub enum IndexerState {
    /// Actively filling gap from current_position to target_end.
    /// Written BEFORE starting historical indexing to enable crash recovery.
    HistoricalIndexing {
        target_end: u32,
        current_position: u32,
    },
    /// Fully synced, processing new blocks as they arrive.
    Active { last_indexed: u32 },
}

/// Indexer error type.
#[derive(Debug, thiserror::Error)]
pub enum IndexerError {
    #[error("Block not found: {0}")]
    BlockNotFound(String),

    #[error("Failed to decode data: {0}")]
    DecodeError(#[from] codec::Error),

    #[error(transparent)]
    Blockchain(#[from] sp_blockchain::Error),
}

/// Add result type alias
pub type Result<T> = std::result::Result<T, IndexerError>;

/// Provides transaction indexing functionality for Bitcoin transactions in Substrate blocks.
///
/// The indexer maintains a mapping of Bitcoin transaction IDs to their positions within blocks,
/// allowing efficient transaction lookups. It handles both new block imports and chain reorganizations.
#[derive(Debug)]
pub struct TransactionIndexer<Block, Backend, Client, TransactionAdapter> {
    network: bitcoin::Network,
    client: Arc<Client>,
    _phantom: PhantomData<(Block, Backend, TransactionAdapter)>,
}

impl<Block, Backend, Client, TransactionAdapter> Clone
    for TransactionIndexer<Block, Backend, Client, TransactionAdapter>
{
    fn clone(&self) -> Self {
        Self {
            network: self.network,
            client: self.client.clone(),
            _phantom: self._phantom,
        }
    }
}

impl<Block, Backend, Client, TransactionAdapter>
    TransactionIndexer<Block, Backend, Client, TransactionAdapter>
where
    Block: BlockT,
    Backend: sc_client_api::backend::Backend<Block>,
    Client: BlockchainEvents<Block>
        + HeaderBackend<Block>
        + BlockBackend<Block>
        + StorageProvider<Block, Backend>
        + AuxStore
        + 'static,
    TransactionAdapter: BitcoinTransactionAdapter<Block>,
{
    /// Creates a new instance of [`TransactionIndexer`].
    pub fn new(
        network: bitcoin::Network,
        client: Arc<Client>,
        spawn_handle: SpawnTaskHandle,
    ) -> Result<Self> {
        if let Some(gap) = Self::detect_index_gap(&client)? {
            let client = client.clone();
            spawn_handle.spawn_blocking("tx-historical-index", None, async move {
                if let Err(err) = index_historical_blocks::<_, TransactionAdapter, _>(client, gap) {
                    tracing::error!(?err, "Failed to index historical blocks");
                }
            });
        }

        Ok(Self {
            network,
            client,
            _phantom: Default::default(),
        })
    }

    /// Detects if there are any gaps in the transaction index.
    ///
    /// This function handles:
    /// 1. Resume interrupted historical indexing (HistoricalIndexing state)
    /// 2. Catch up if fell behind (Active state with last_indexed < best)
    /// 3. Fresh start on existing chain (no state - index all blocks)
    fn detect_index_gap(client: &Client) -> Result<Option<IndexRange>> {
        let best_number: u32 = client.info().best_number.saturated_into();

        match load_indexer_state(client)? {
            Some(IndexerState::HistoricalIndexing {
                target_end,
                current_position,
            }) => {
                // Resume interrupted historical indexing
                tracing::info!(
                    current_position,
                    target_end,
                    "Resuming interrupted transaction indexing"
                );
                Ok(Some(current_position..target_end))
            }
            Some(IndexerState::Active { last_indexed }) => {
                // Check if we fell behind (node was offline while chain grew)
                if last_indexed < best_number {
                    tracing::info!(
                        last_indexed,
                        best_number,
                        "Transaction index behind, catching up"
                    );
                    Ok(Some(last_indexed + 1..best_number + 1))
                } else {
                    Ok(None)
                }
            }
            None => {
                // Fresh start - index all existing blocks
                if best_number > 0 {
                    tracing::info!(
                        best_number,
                        "First run with --tx-index, indexing all {} existing blocks",
                        best_number + 1
                    );

                    // Write state BEFORE starting (enables crash recovery)
                    write_indexer_state(
                        client,
                        &IndexerState::HistoricalIndexing {
                            target_end: best_number + 1,
                            current_position: 0,
                        },
                    )?;

                    Ok(Some(0..best_number + 1))
                } else {
                    // No blocks exist yet - start fresh
                    write_indexer_state(client, &IndexerState::Active { last_indexed: 0 })?;
                    Ok(None)
                }
            }
        }
    }

    pub async fn run(self) {
        let mut block_import_stream = self.client.every_import_notification_stream();

        while let Some(notification) = block_import_stream.next().await {
            let Ok(Some(SignedBlock {
                block,
                justifications: _,
            })) = self.client.block(notification.hash)
            else {
                tracing::error!(hash = ?notification.hash, "Imported block unavailable");
                continue;
            };

            let res = if let Some(route) = notification.tree_route {
                self.handle_reorg(route)
            } else {
                self.handle_new_block(block)
            };

            if let Err(err) = res {
                tracing::error!(
                    ?err,
                    hash = ?notification.hash,
                    "Failed to index block, continuing..."
                );
                // Don't panic - log and continue processing
                // The indexer may fall behind but will catch up on next restart
            }
        }
    }

    /// Handles retracted and enacted blocks during a re-org.
    fn handle_reorg(&self, route: Arc<sp_blockchain::TreeRoute<Block>>) -> Result<()> {
        for hash_and_number in route.retracted() {
            let block = self.get_block(hash_and_number.hash)?;
            process_block::<_, TransactionAdapter, _>(&*self.client, block, IndexAction::Revert);
        }

        for hash_and_number in route.enacted() {
            let block = self.get_block(hash_and_number.hash)?;
            process_block::<_, TransactionAdapter, _>(&*self.client, block, IndexAction::Apply);
        }

        Ok(())
    }

    /// Handles a new block import (non-reorg).
    fn handle_new_block(&self, block: Block) -> Result<()> {
        let block_number: u32 = (*block.header().number()).saturated_into();

        process_block::<_, TransactionAdapter, _>(&*self.client, block, IndexAction::Apply);

        // Update state to track last indexed block
        write_indexer_state(
            &*self.client,
            &IndexerState::Active {
                last_indexed: block_number,
            },
        )?;

        Ok(())
    }

    fn get_block(&self, block_hash: Block::Hash) -> Result<Block> {
        self.client
            .block(block_hash)?
            .ok_or_else(|| IndexerError::BlockNotFound(format!("{block_hash:?}")))
            .map(|signed| signed.block)
    }
}

/// How often to save progress during historical indexing (every N blocks).
const PROGRESS_SAVE_INTERVAL: u32 = 1000;

fn index_historical_blocks<Block, TransactionAdapter, Client>(
    client: Arc<Client>,
    gap_range: IndexRange,
) -> sp_blockchain::Result<()>
where
    Block: BlockT,
    TransactionAdapter: BitcoinTransactionAdapter<Block>,
    Client: BlockBackend<Block> + HeaderBackend<Block> + AuxStore,
{
    let total_blocks = gap_range.end.saturating_sub(gap_range.start);
    let start_time = std::time::Instant::now();
    let mut processed = 0u32;

    tracing::info!(
        start = gap_range.start,
        end = gap_range.end,
        total = total_blocks,
        "Starting historical transaction indexing"
    );

    for block_number in gap_range.clone() {
        let block_hash = client.hash(block_number.into())?.ok_or_else(|| {
            sp_blockchain::Error::Backend(format!("Hash for block#{block_number} not found"))
        })?;
        let block = client
            .block(block_hash)?
            .ok_or_else(|| {
                sp_blockchain::Error::Backend(format!(
                    "Missing block#{block_number},{block_hash:?}"
                ))
            })?
            .block;

        process_block::<_, TransactionAdapter, _>(&*client, block, IndexAction::Apply);
        processed += 1;

        // Save progress periodically and on last block
        let is_last = block_number == gap_range.end.saturating_sub(1);
        if processed % PROGRESS_SAVE_INTERVAL == 0 || is_last {
            let current_position = block_number + 1;

            if !is_last {
                // Update state for crash recovery
                write_indexer_state(
                    &*client,
                    &IndexerState::HistoricalIndexing {
                        target_end: gap_range.end,
                        current_position,
                    },
                )?;
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
                "Transaction indexing progress"
            );
        }
    }

    // Transition to Active state
    let last_indexed = gap_range.end.saturating_sub(1);
    write_indexer_state(&*client, &IndexerState::Active { last_indexed })?;

    tracing::info!(
        blocks = total_blocks,
        duration_secs = start_time.elapsed().as_secs(),
        "Historical transaction indexing complete"
    );

    Ok(())
}

fn process_block<Block, TransactionAdapter, B>(backend: &B, block: Block, index_action: IndexAction)
where
    Block: BlockT,
    TransactionAdapter: BitcoinTransactionAdapter<Block>,
    B: AuxStore,
{
    let block_number: u32 = (*block.header().number()).saturated_into();
    let bitcoin_block =
        subcoin_primitives::convert_to_bitcoin_block::<Block, TransactionAdapter>(block)
            .expect("Failed to convert Substrate block to Bitcoin block");
    let changes = bitcoin_block
        .txdata
        .iter()
        .enumerate()
        .map(|(index, tx)| {
            (
                tx.compute_txid(),
                TxPosition {
                    block_number,
                    index: index as u32,
                },
            )
        })
        .collect::<Vec<_>>();
    if let Err(err) = write_transaction_index_changes(backend, index_action, changes) {
        tracing::error!(?err, "Failed to write index changes");
    }
}

fn load_decode<B, T>(backend: &B, key: &[u8]) -> sp_blockchain::Result<Option<T>>
where
    B: AuxStore,
    T: Decode,
{
    match backend.get_aux(key)? {
        Some(t) => T::decode(&mut &t[..]).map(Some).map_err(|e: codec::Error| {
            sp_blockchain::Error::Backend(format!("Subcoin DB is corrupted. Decode error: {e}"))
        }),
        None => Ok(None),
    }
}

fn load_indexer_state<B: AuxStore>(backend: &B) -> sp_blockchain::Result<Option<IndexerState>> {
    load_decode(backend, INDEXER_STATE_KEY)
}

fn write_indexer_state<B: AuxStore>(
    backend: &B,
    state: &IndexerState,
) -> sp_blockchain::Result<()> {
    backend.insert_aux(&[(INDEXER_STATE_KEY, state.encode().as_slice())], &[])
}

fn write_transaction_index_changes<B: AuxStore>(
    backend: &B,
    index_action: IndexAction,
    changes: Vec<(Txid, TxPosition)>,
) -> sp_blockchain::Result<()> {
    match index_action {
        IndexAction::Apply => {
            let key_values = changes
                .iter()
                .map(|(txid, tx_pos)| (txid_key(*txid), tx_pos.encode()))
                .collect::<Vec<_>>();
            backend.insert_aux(
                key_values
                    .iter()
                    .map(|(k, v)| (k.as_slice(), v.as_slice()))
                    .collect::<Vec<_>>()
                    .iter(),
                &[],
            )
        }
        IndexAction::Revert => {
            let keys = changes
                .iter()
                .map(|(txid, _tx_pos)| txid_key(*txid))
                .collect::<Vec<_>>();
            backend.insert_aux(
                &[],
                keys.iter().map(|k| k.as_slice()).collect::<Vec<_>>().iter(),
            )
        }
    }
}

pub struct TransactionIndexProvider<Client> {
    client: Arc<Client>,
}

impl<Client> TransactionIndexProvider<Client> {
    pub fn new(client: Arc<Client>) -> Self {
        Self { client }
    }
}

impl<Client> TransactionIndex for TransactionIndexProvider<Client>
where
    Client: AuxStore,
{
    fn tx_index(&self, txid: Txid) -> sp_blockchain::Result<Option<TxPosition>> {
        load_transaction_index(&*self.client, txid)
    }
}

fn txid_key(txid: Txid) -> Vec<u8> {
    (b"txid", txid.as_byte_array()).encode()
}

fn load_transaction_index<B: AuxStore>(
    backend: &B,
    txid: Txid,
) -> sp_blockchain::Result<Option<TxPosition>> {
    load_decode(backend, &txid_key(txid))
}
