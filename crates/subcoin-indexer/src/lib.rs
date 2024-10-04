use bitcoin::{Address, BlockHash, OutPoint, Script, Transaction, TxOut};
use codec::Decode;
use futures::StreamExt;
use sc_client_api::{BlockBackend, BlockchainEvents, StorageProvider};
use sp_runtime::generic::SignedBlock;
use sp_runtime::traits::{Block as BlockT, Header};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;
use subcoin_primitives::runtime::Coin;
use subcoin_primitives::{BitcoinTransactionAdapter, CoinStorageKey};

struct IndexerStore {
    network: bitcoin::Network,
    balances: HashMap<Address, u64>,
}

impl IndexerStore {
    fn new(network: bitcoin::Network) -> Self {
        Self {
            network,
            balances: HashMap::new(),
        }
    }

    fn apply_tx_changes(&mut self, added_utxos: Vec<TxOut>, spent_coins: Vec<Coin>) {
        // Add UTXOs to the indexer.
        for txout in added_utxos {
            if let Some(address) =
                Address::from_script(txout.script_pubkey.as_script(), self.network).ok()
            {
                let value = txout.value.to_sat();
                self.balances
                    .entry(address)
                    .and_modify(|e| {
                        *e += value;
                    })
                    .or_insert(value);
            }
        }

        // Remove spent coins.
        for coin in spent_coins {
            if let Some(address) =
                Address::from_script(Script::from_bytes(&coin.script_pubkey), self.network).ok()
            {
                self.balances.entry(address).and_modify(|e| {
                    *e -= coin.amount;
                });
            }
        }
    }

    fn undo_tx_changes(&mut self, added_utxos: Vec<TxOut>, spent_coins: Vec<Coin>) {
        // Remove added UTXOs.
        for txout in added_utxos {
            if let Some(address) =
                Address::from_script(txout.script_pubkey.as_script(), self.network).ok()
            {
                let value = txout.value.to_sat();
                self.balances.entry(address).and_modify(|e| {
                    *e -= value;
                });
            }
        }

        // Restore spent coins.
        for coin in spent_coins {
            if let Some(address) =
                Address::from_script(Script::from_bytes(&coin.script_pubkey), self.network).ok()
            {
                self.balances.entry(address).and_modify(|e| {
                    *e += coin.amount;
                });
            }
        }
    }
}

/// Indexer responsible for tracking BTC balances by address.
pub struct BtcBalanceIndexer<Block, Backend, Client, TransactionAdapter> {
    network: bitcoin::Network,
    client: Arc<Client>,
    coin_storage_key: Arc<dyn CoinStorageKey>,
    _phantom: PhantomData<(Block, Backend, TransactionAdapter)>,
}

impl<Block, Backend, Client, TransactionAdapter>
    BtcBalanceIndexer<Block, Backend, Client, TransactionAdapter>
where
    Block: BlockT,
    Backend: sc_client_api::backend::Backend<Block>,
    Client: BlockchainEvents<Block> + BlockBackend<Block> + StorageProvider<Block, Backend>,
    TransactionAdapter: BitcoinTransactionAdapter<Block>,
{
    pub async fn run(&self) {
        let mut indexer_store = IndexerStore::new(self.network);

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
            if let Some(route) = notification.tree_route {
                // Rollback the retracted blocks.
                for hash_and_number in route.retracted() {
                    let block = self.expect_block(hash_and_number.hash);
                    self.undo_block_changes(block, &mut indexer_store);
                }

                // Process the enacted blocks.
                for hash_and_number in route.enacted() {
                    let block = self.expect_block(hash_and_number.hash);
                    self.apply_block_changes(block, &mut indexer_store);
                }
            } else {
                self.apply_block_changes(block, &mut indexer_store);
            }
        }
    }

    fn expect_coin_at(&self, block_hash: Block::Hash, out_point: OutPoint) -> Coin {
        let OutPoint { txid, vout } = out_point;

        let storage_key = self.coin_storage_key.storage_key(txid, vout);

        self.client
            .storage(block_hash, &sc_client_api::StorageKey(storage_key))
            .ok()
            .flatten()
            .and_then(|data| Coin::decode(&mut data.0.as_slice()).ok())
            .expect("Coin must exist in the parent block's state")
    }

    fn expect_block(&self, block_hash: Block::Hash) -> Block {
        self.client
            .block(block_hash)
            .ok()
            .flatten()
            .expect("Missing block {block_hash:?}")
            .block
    }

    fn apply_block_changes(&self, block: Block, indexer_store: &mut IndexerStore) {
        let txdata = block
            .extrinsics()
            .iter()
            .map(TransactionAdapter::extrinsic_to_bitcoin_transaction)
            .collect::<Vec<_>>();

        let parent_hash = *block.header().parent_hash();

        for Transaction { input, output, .. } in txdata {
            let spent_coins = input
                .iter()
                .map(|txin| self.expect_coin_at(parent_hash, txin.previous_output))
                .collect::<Vec<_>>();

            indexer_store.apply_tx_changes(output, spent_coins);
        }
    }

    fn undo_block_changes(&self, block: Block, indexer_store: &mut IndexerStore) {
        let txdata = block
            .extrinsics()
            .iter()
            .map(TransactionAdapter::extrinsic_to_bitcoin_transaction)
            .collect::<Vec<_>>();

        let parent_hash = *block.header().parent_hash();

        for Transaction { input, output, .. } in txdata {
            let spent_coins = input
                .iter()
                .map(|txin| self.expect_coin_at(parent_hash, txin.previous_output))
                .collect::<Vec<_>>();

            indexer_store.undo_tx_changes(output, spent_coins);
        }
    }
}
