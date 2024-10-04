mod indexers;

use bitcoin::{Address, Script, TxOut};
use sc_client_api::StorageProvider;
use std::collections::HashMap;
use subcoin_primitives::runtime::Coin;

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
