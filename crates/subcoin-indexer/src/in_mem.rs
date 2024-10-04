use crate::BalanceChanges;
use bitcoin::Address;
use std::collections::HashMap;

pub struct InMemStore {
    balances: HashMap<Address, u64>,
}

impl InMemStore {
    pub fn new() -> Self {
        Self {
            balances: HashMap::new(),
        }
    }

    pub fn write_balance_changes(&mut self, balance_changes: BalanceChanges) {
        let BalanceChanges {
            to_increase,
            to_decrease,
        } = balance_changes;

        for (address, value) in to_increase {
            self.balances
                .entry(address)
                .and_modify(|e| {
                    *e += value;
                })
                .or_insert(value);
        }

        for (address, value) in to_decrease {
            self.balances.entry(address).and_modify(|e| {
                *e -= value;
            });
        }
    }
}
