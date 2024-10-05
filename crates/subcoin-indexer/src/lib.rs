mod in_mem;
mod indexers;
mod postgres;

use bitcoin::{Address, Script, TxOut};
use std::collections::HashMap;
use subcoin_primitives::runtime::Coin;

pub use indexers::btc::BtcIndexer;
pub use indexers::BackendType;

#[derive(Debug)]
enum BlockAction {
    ApplyNew,
    Undo,
}

#[derive(Debug)]
struct BalanceChanges {
    to_increase: HashMap<Address, u64>,
    to_decrease: HashMap<Address, u64>,
}

impl BalanceChanges {
    pub fn new() -> Self {
        Self {
            to_increase: HashMap::new(),
            to_decrease: HashMap::new(),
        }
    }

    /// Merge another [`BalanceChanges`] into this one.
    pub fn merge(&mut self, other: BalanceChanges) {
        // Merge increase changes
        for (address, amount) in other.to_increase {
            *self.to_increase.entry(address).or_insert(0) += amount;
        }

        // Merge decrease changes
        for (address, amount) in other.to_decrease {
            *self.to_decrease.entry(address).or_insert(0) += amount;
        }
    }
}

struct TxInfo {
    new_utxos: Vec<TxOut>,
    spent_coins: Vec<Coin>,
}

fn calculate_transaction_balance_changes(
    network: bitcoin::Network,
    action: BlockAction,
    tx_info: TxInfo,
) -> BalanceChanges {
    let TxInfo {
        new_utxos,
        spent_coins,
    } = tx_info;

    let utxo_changes = new_utxos
        .into_iter()
        .filter_map(|txout| {
            if let Some(address) =
                Address::from_script(txout.script_pubkey.as_script(), network).ok()
            {
                Some((address, txout.value.to_sat()))
            } else {
                None
            }
        })
        .collect::<HashMap<_, _>>();

    let coin_changes = spent_coins
        .into_iter()
        .filter_map(|coin| {
            if let Some(address) =
                Address::from_script(Script::from_bytes(&coin.script_pubkey), network).ok()
            {
                Some((address, coin.amount))
            } else {
                None
            }
        })
        .collect::<HashMap<_, _>>();

    match action {
        BlockAction::ApplyNew => BalanceChanges {
            to_increase: utxo_changes,
            to_decrease: coin_changes,
        },
        BlockAction::Undo => BalanceChanges {
            to_increase: coin_changes,
            to_decrease: utxo_changes,
        },
    }
}
