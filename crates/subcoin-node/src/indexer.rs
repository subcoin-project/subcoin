use bitcoin::{Address, OutPoint, Script, Transaction};
use codec::Decode;
use futures::StreamExt;
use sc_client_api::{BlockBackend, BlockchainEvents, StorageProvider};
use sp_runtime::traits::{Block as BlockT, Header};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;
use subcoin_primitives::runtime::Coin;
use subcoin_primitives::{BitcoinTransactionAdapter, CoinStorageKey};

/// Indexer responsible for tracking BTC balances by address.
pub struct BtcBalanceIndexer<Block, Backend, Client> {
    network: bitcoin::Network,
    client: Arc<Client>,
    _phantom: PhantomData<(Block, Backend)>,
}

impl<Block, Backend, Client> BtcBalanceIndexer<Block, Backend, Client>
where
    Block: BlockT,
    Backend: sc_client_api::backend::Backend<Block>,
    Client: BlockchainEvents<Block> + BlockBackend<Block> + StorageProvider<Block, Backend>,
{
    pub async fn run(&self) {
        let mut block_import_stream = self.client.every_import_notification_stream();

        let mut balances = HashMap::new();

        while let Some(notification) = block_import_stream.next().await {
            let Ok(Some(substrate_block)) = self.client.block(notification.hash) else {
                tracing::error!("Imported block {} unavailable", notification.hash);
                continue;
            };

            let block = substrate_block.block;

            let txdata = block
                .extrinsics()
                .iter()
                .map(<subcoin_service::TransactionAdapter as BitcoinTransactionAdapter::<Block>>::extrinsic_to_bitcoin_transaction)
                .collect::<Vec<_>>();

            for Transaction { input, output, .. } in txdata {
                // Process output.
                for txout in output {
                    if let Some(address) =
                        Address::from_script(txout.script_pubkey.as_script(), self.network).ok()
                    {
                        let value = txout.value.to_sat();
                        balances
                            .entry(address)
                            .and_modify(|e| {
                                *e += value;
                            })
                            .or_insert(value);
                    }
                }

                // Process input.
                let parent_hash = block.header().parent_hash();

                for txin in input {
                    let OutPoint { txid, vout } = txin.previous_output;

                    let storage_key = subcoin_service::CoinStorageKey.storage_key(txid, vout);

                    let spent_coin = self
                        .client
                        .storage(*parent_hash, &sc_client_api::StorageKey(storage_key))
                        .ok()
                        .flatten()
                        .and_then(|data| Coin::decode(&mut data.0.as_slice()).ok())
                        .expect("Spent coin must exist in parent state");

                    if let Some(address) = Address::from_script(
                        Script::from_bytes(&spent_coin.script_pubkey),
                        self.network,
                    )
                    .ok()
                    {
                        balances.entry(address).and_modify(|e| {
                            *e -= spent_coin.amount;
                        });
                    }
                }
            }
        }
    }
}
