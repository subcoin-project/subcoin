use crate::{calculate_transaction_balance_changes, BalanceChanges, BlockAction, TxInfo};
use bitcoin::{OutPoint, Transaction, TxIn};
use codec::Decode;
use futures::StreamExt;
use sc_client_api::{BlockBackend, BlockchainEvents, StorageProvider};
use sp_runtime::generic::SignedBlock;
use sp_runtime::traits::{Block as BlockT, Header};
use std::marker::PhantomData;
use std::sync::Arc;
use subcoin_primitives::runtime::Coin;
use subcoin_primitives::{BitcoinTransactionAdapter, CoinStorageKey};

use super::{BackendType, IndexerStore};

/// Indexer responsible for tracking BTC balances by address.
pub struct BtcIndexer<Block, Backend, Client, TransactionAdapter> {
    network: bitcoin::Network,
    client: Arc<Client>,
    coin_storage_key: Arc<dyn CoinStorageKey>,
    indexer_store: IndexerStore,
    _phantom: PhantomData<(Block, Backend, TransactionAdapter)>,
}

impl<Block, Backend, Client, TransactionAdapter>
    BtcIndexer<Block, Backend, Client, TransactionAdapter>
where
    Block: BlockT,
    Backend: sc_client_api::backend::Backend<Block>,
    Client: BlockchainEvents<Block> + BlockBackend<Block> + StorageProvider<Block, Backend>,
    TransactionAdapter: BitcoinTransactionAdapter<Block>,
{
    pub fn new(
        network: bitcoin::Network,
        client: Arc<Client>,
        coin_storage_key: Arc<dyn CoinStorageKey>,
        backend_type: BackendType,
    ) -> Self {
        Self {
            network,
            client,
            coin_storage_key,
            indexer_store: IndexerStore::new(backend_type),
            _phantom: Default::default(),
        }
    }

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
                    self.undo_block_changes(block);
                }

                for hash_and_number in route.enacted() {
                    let block = self.expect_block(hash_and_number.hash);
                    self.apply_block_changes(block);
                }
            } else {
                self.apply_block_changes(block);
            }
        }
    }

    fn get_coin_from_storage(&self, block_hash: Block::Hash, out_point: OutPoint) -> Option<Coin> {
        let OutPoint { txid, vout } = out_point;

        let storage_key = self.coin_storage_key.storage_key(txid, vout);

        self.client
            .storage(block_hash, &sc_client_api::StorageKey(storage_key))
            .ok()
            .flatten()
            .and_then(|data| Coin::decode(&mut data.0.as_slice()).ok())
    }

    fn fetch_spent_coins(
        &self,
        block: &bitcoin::Block,
        parent_hash: Block::Hash,
        input: &[TxIn],
    ) -> Vec<Coin> {
        input
            .iter()
            .map(
                |txin| match self.get_coin_from_storage(parent_hash, txin.previous_output) {
                    Some(coin) => coin,
                    None => find_coin_in_current_block(block, txin.previous_output).unwrap_or_else(
                        || panic!("Coin not found, parent_hash: #{parent_hash:?}, txin: {txin:?}"),
                    ),
                },
            )
            .collect::<Vec<_>>()
    }

    fn expect_block(&self, block_hash: Block::Hash) -> Block {
        self.client
            .block(block_hash)
            .ok()
            .flatten()
            .expect("Missing block {block_hash:?}")
            .block
    }

    fn apply_block_changes(&mut self, block: Block) {
        self.process_block(block, BlockAction::ApplyNew)
    }

    fn undo_block_changes(&mut self, block: Block) {
        self.process_block(block, BlockAction::Undo)
    }

    fn process_block(&mut self, block: Block, block_action: BlockAction) {
        let parent_hash = *block.header().parent_hash();

        let bitcoin_block =
            subcoin_primitives::convert_to_bitcoin_block::<Block, TransactionAdapter>(block)
                .expect("Failed to convert Substrate block to Bitcoin block");

        let mut block_changes = BalanceChanges::new();

        for tx in &bitcoin_block.txdata {
            let is_coinbase = tx.is_coinbase();

            let Transaction { input, output, .. } = tx;

            let tx_changes = calculate_transaction_balance_changes(
                self.network,
                block_action,
                TxInfo {
                    new_utxos: output.to_vec(),
                    spent_coins: if is_coinbase {
                        Vec::new()
                    } else {
                        self.fetch_spent_coins(&bitcoin_block, parent_hash, input)
                    },
                },
            );

            block_changes.merge(tx_changes);
        }

        self.indexer_store.write_block_changes(block_changes);
    }
}

fn extract_transactions<
    'a,
    Block: BlockT,
    TransactionAdapter: BitcoinTransactionAdapter<Block> + 'a,
>(
    block: &'a Block,
) -> impl Iterator<Item = Transaction> + '_ {
    block
        .extrinsics()
        .iter()
        .map(TransactionAdapter::extrinsic_to_bitcoin_transaction)
}

fn find_coin_in_current_block(block: &bitcoin::Block, out_point: OutPoint) -> Option<Coin> {
    let OutPoint { txid, vout } = out_point;
    block
        .txdata
        .iter()
        .enumerate()
        .find_map(|(index, tx)| (tx.compute_txid() == txid).then_some((tx, index == 0)))
        .and_then(|(tx, is_coinbase)| {
            tx.output.get(vout as usize).cloned().map(|txout| Coin {
                is_coinbase,
                amount: txout.value.to_sat(),
                height: 0u32, // TODO: unused
                script_pubkey: txout.script_pubkey.to_bytes(),
            })
        })
}
