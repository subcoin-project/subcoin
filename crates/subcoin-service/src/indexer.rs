use bitcoin::hashes::Hash;
use bitcoin::Txid;
use codec::{Decode, Encode};
use futures::StreamExt;
use sc_client_api::backend::AuxStore;
use sc_client_api::{BlockBackend, BlockchainEvents, StorageProvider};
use sp_runtime::generic::SignedBlock;
use sp_runtime::traits::{Block as BlockT, Header, SaturatedConversion};
use std::marker::PhantomData;
use std::sync::Arc;
use subcoin_primitives::{BitcoinTransactionAdapter, TransactionIndex, TxPosition};

#[derive(Debug, Clone, Copy)]
enum BlockAction {
    ApplyNew,
    Undo,
}

/// Indexer responsible for tracking transactions so that it is possible to query a transaction
/// by the corresponding txid.
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

#[derive(Debug, Encode, Decode)]
struct HistoricalIndexing {
    // inclusive
    begin: u32,
    // inclusive
    end: u32,
}

impl<Block, Backend, Client, TransactionAdapter>
    TransactionIndexer<Block, Backend, Client, TransactionAdapter>
where
    Block: BlockT,
    Backend: sc_client_api::backend::Backend<Block>,
    Client:
        BlockchainEvents<Block> + BlockBackend<Block> + StorageProvider<Block, Backend> + AuxStore,
    TransactionAdapter: BitcoinTransactionAdapter<Block>,
{
    /// Creates a new instance of [`TransactionIndexer`].
    pub fn new(network: bitcoin::Network, client: Arc<Client>) -> Self {
        let best_number = self.client.info().best_number;

        let begin = match load_last_indexed_historical_block(&*self.client)? {
            Some(last_indexed) => last_indexed,
            None => begin,
        };

        let HistoricalIndexing { begin, end } = todo!("load sync range");

        Self {
            network,
            client,
            _phantom: Default::default(),
        }
    }

    /// Listen for new coming blocks and index them in real-time.
    pub async fn run(mut self) {
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

            // Re-org occurs.
            //
            // Rollback the retracted blocks and then process the enacted blocks.
            if let Some(route) = notification.tree_route {
                for hash_and_number in route.retracted() {
                    let block = self.expect_block(hash_and_number.hash);
                    self.process_block(block, BlockAction::Undo)
                }

                for hash_and_number in route.enacted() {
                    let block = self.expect_block(hash_and_number.hash);
                    self.process_block(block, BlockAction::ApplyNew)
                }
            } else {
                self.process_block(block, BlockAction::ApplyNew)
            }
        }
    }

    fn index_historical_blocks(self) -> sp_blockchain::Result<()> {
        // TODO: handle the historical blocks' indexing.
        let HistoricalIndexing { begin, end } = todo!("load sync range");

        let begin = match load_last_indexed_historical_block(&*self.client)? {
            Some(last_indexed) => last_indexed,
            None => begin,
        };

        // Scan backward the unindexed blocks.
        for block_number in (end..=begin).rev() {
            let substrate_block_hash =
                self.client
                    .hash(block_number.into())?
                    .ok_or(sp_blockchain::Error::Backend(format!(
                        "Hash for #{block_number} not found"
                    )))?;
            let substrate_block = self.expect_block(substrate_block_hash);
            self.process_block(substrate_block, BlockAction::ApplyNew);
            self.write_last_indexed_historical_block(block_number);
        }

        // Historical sync is complete, remove the stored historical sync state.
        delete_historical_indexing(&*self.client);
        delete_last_indexed_historical_block(&*self.client);

        Ok(())
    }

    fn expect_block(&self, block_hash: Block::Hash) -> Block {
        self.client
            .block(block_hash)
            .ok()
            .flatten()
            .expect("Missing block {block_hash:?}")
            .block
    }

    fn process_block(&mut self, block: Block, block_action: BlockAction) {
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

        if let Err(err) = write_transaction_index_changes(&*self.client, block_action, changes) {
            tracing::error!(?err, "Failed to write index changes");
        }
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

fn txid_key(txid: Txid) -> Vec<u8> {
    (b"txid", txid.as_byte_array()).encode()
}

fn write_transaction_index_changes<B: AuxStore>(
    backend: &B,
    block_action: BlockAction,
    changes: Vec<(Txid, TxPosition)>,
) -> sp_blockchain::Result<()> {
    match block_action {
        BlockAction::ApplyNew => {
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
        BlockAction::Undo => {
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

fn write_last_indexed_historical_block<B: AuxStore>(
    backend: &B,
    block_number: u32,
) -> sp_blockchain::Result<()> {
    backend.insert_aux(
        &[(b"last_indexed_historical_block", block_number.encode())],
        &[],
    )
}

fn delete_last_indexed_historical_block<B: AuxStore>(backend: &B) -> sp_blockchain::Result<()> {
    backend.insert_aux(&[], &[b"last_indexed_historical_block"])
}

fn load_last_indexed_historical_block<B: AuxStore>(
    backend: &B,
) -> sp_blockchain::Result<Option<u32>> {
    load_decode(backend, b"last_indexed_historical_block")
}

fn load_historical_indexing<B: AuxStore>(
    backend: &B,
) -> sp_blockchain::Result<Option<HistoricalIndexing>> {
    load_decode(backend, b"historical_indexing")
}

fn delete_historical_indexing<B: AuxStore>(
    backend: &B,
) -> sp_blockchain::Result<Option<HistoricalIndexing>> {
    backend.insert_aux(&[], &[b"historical_indexing"])
}

fn load_transaction_index<B: AuxStore>(
    backend: &B,
    txid: Txid,
) -> sp_blockchain::Result<Option<TxPosition>> {
    load_decode(backend, &txid_key(txid))
}

impl<Block, Backend, Client, TransactionAdapter> TransactionIndex
    for TransactionIndexer<Block, Backend, Client, TransactionAdapter>
where
    Block: BlockT,
    Backend: sc_client_api::backend::Backend<Block>,
    Client:
        BlockchainEvents<Block> + BlockBackend<Block> + StorageProvider<Block, Backend> + AuxStore,
    TransactionAdapter: BitcoinTransactionAdapter<Block>,
{
    fn tx_index(&self, txid: Txid) -> sp_blockchain::Result<Option<TxPosition>> {
        load_transaction_index(&*self.client, txid)
    }
}
