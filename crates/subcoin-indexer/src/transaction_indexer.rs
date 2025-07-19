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

const TX_INDEX_GAP_KEY: &[u8] = b"tx_index_gap";

const INDEXED_BLOCK_RANGE_KEY: &[u8] = b"tx_indexed_block_range";

type IndexRange = std::ops::Range<u32>;

/// The range of indexed blocks.
/// - `start`: The first block number indexed.
/// - `end`: One past the last indexed block.
type IndexedBlockRange = IndexRange;

/// Represents actions applied to a block during transaction indexing.
#[derive(Debug, Clone, Copy)]
enum IndexAction {
    Apply,
    Revert,
}

/// Indexer error type.
#[derive(Debug, thiserror::Error)]
pub enum IndexerError {
    #[error("Block not found: {0}")]
    BlockNotFound(String),

    #[error("Inconsistent block range. Indexed: {indexed:?}, Processed: {processed}")]
    InconsistentBlockRange {
        indexed: Option<IndexedBlockRange>,
        processed: u32,
    },

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
    fn detect_index_gap(client: &Client) -> Result<Option<IndexRange>> {
        let gap = if let Some(gap) = load_index_gap(client)? {
            Some(gap)
        } else if let Some(ref block_range) = load_indexed_block_range(client)? {
            let best_number: u32 = client.info().best_number.saturated_into();
            let last_indexed_block = block_range.end.saturating_sub(1);

            if last_indexed_block < best_number {
                let new_gap = last_indexed_block + 1..best_number + 1;
                tracing::debug!(
                    last_indexed = last_indexed_block,
                    best_number = best_number,
                    ?new_gap,
                    "Detected transaction indexing gap"
                );
                Some(new_gap)
            } else {
                None
            }
        } else {
            None
        };

        Ok(gap)
    }

    pub async fn run(self) {
        let mut block_import_stream = self.client.every_import_notification_stream();

        while let Some(notification) = block_import_stream.next().await {
            let Ok(Some(SignedBlock {
                block,
                justifications: _,
            })) = self.client.block(notification.hash)
            else {
                tracing::error!("Imported block {} unavailable", notification.hash);
                continue;
            };

            let res = if let Some(route) = notification.tree_route {
                self.handle_reorg(route)
            } else {
                self.handle_new_block(block)
            };

            if let Err(err) = res {
                panic!("Failed to process block#{}: {err:?}", notification.hash);
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

        let mut indexed_block_range = load_indexed_block_range(&*self.client)?;

        match indexed_block_range.as_mut() {
            Some(current_range) => {
                if current_range.end == block_number {
                    current_range.end += 1;
                    write_tx_indexed_range(&*self.client, current_range.encode())?;
                } else {
                    return Err(IndexerError::InconsistentBlockRange {
                        indexed: indexed_block_range,
                        processed: block_number,
                    });
                }
            }
            None => {
                let new_range = block_number..block_number + 1;
                write_tx_indexed_range(&*self.client, new_range.encode())?;
            }
        }

        Ok(())
    }

    fn get_block(&self, block_hash: Block::Hash) -> Result<Block> {
        self.client
            .block(block_hash)?
            .ok_or_else(|| IndexerError::BlockNotFound(format!("{block_hash:?}")))
            .map(|signed| signed.block)
    }
}

fn index_historical_blocks<Block, TransactionAdapter, Client>(
    client: Arc<Client>,
    gap_range: IndexRange,
) -> sp_blockchain::Result<()>
where
    Block: BlockT,
    TransactionAdapter: BitcoinTransactionAdapter<Block>,
    Client: BlockBackend<Block> + HeaderBackend<Block> + AuxStore,
{
    let mut remaining_gap = gap_range.clone();

    tracing::debug!("Starting to index historical blocks in range {gap_range:?}");

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

        remaining_gap.start += 1;
        write_index_gap(&*client, remaining_gap.encode())?;
    }

    tracing::debug!("Finished indexing historical blocks. Final gap status: {remaining_gap:?}");

    delete_index_gap(&*client)?;

    // Extends the existing indexed block range or initializes a new range.
    match load_indexed_block_range(&*client)? {
        Some(mut existing_range) => {
            // Extend the range if there is overlap or new blocks beyond the current range.
            if gap_range.end > existing_range.end {
                existing_range.end = gap_range.end;
                write_tx_indexed_range(&*client, existing_range.encode())?;
            }
        }
        None => {
            tracing::debug!("No prior range exist; initialize new gap range: {gap_range:?}");
            write_tx_indexed_range(&*client, gap_range.encode())?;
        }
    }

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

fn load_indexed_block_range<B: AuxStore>(
    backend: &B,
) -> sp_blockchain::Result<Option<IndexedBlockRange>> {
    load_decode(backend, INDEXED_BLOCK_RANGE_KEY)
}

fn write_tx_indexed_range<B: AuxStore>(
    backend: &B,
    encoded_indexed_block_range: Vec<u8>,
) -> sp_blockchain::Result<()> {
    backend.insert_aux(
        &[(
            INDEXED_BLOCK_RANGE_KEY,
            encoded_indexed_block_range.as_slice(),
        )],
        &[],
    )
}

fn load_index_gap<B: AuxStore>(backend: &B) -> sp_blockchain::Result<Option<IndexRange>> {
    load_decode(backend, TX_INDEX_GAP_KEY)
}

fn write_index_gap<B: AuxStore>(backend: &B, encoded_gap: Vec<u8>) -> sp_blockchain::Result<()> {
    backend.insert_aux(&[(TX_INDEX_GAP_KEY, encoded_gap.as_slice())], &[])
}

fn delete_index_gap<B: AuxStore>(backend: &B) -> sp_blockchain::Result<()> {
    backend.insert_aux([], &[TX_INDEX_GAP_KEY])
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
