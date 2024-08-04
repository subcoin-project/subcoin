use crate::PeerId;
use bitcoin::consensus::deserialize;
use bitcoin::{Transaction, Txid};
use indexmap::map::Entry;
use indexmap::IndexMap;
use std::collections::HashSet;
use std::time::Instant;

#[derive(Debug)]
struct TransactionInfo {
    /// The actual transaction to be sent to the network.
    transaction: Transaction,
    /// Set of peers to which we advertised this transaction.
    ///
    /// Note that having a peer in this set doesn't guarantee the the peer actually
    /// received the transaction.
    advertised: HashSet<PeerId>,
    /// Time at which the transaction was added.
    at: Instant,
}

/// This struct manages the transactions received from the network.
#[derive(Debug)]
pub(crate) struct TransactionManager {
    /// List of transactions tracked by this manager, in the FIFO order.
    transactions: IndexMap<Txid, TransactionInfo>,
}

impl TransactionManager {
    /// Maximum number of transactions the manager holds.
    const MAX_TRANSACTIONS: usize = 256;

    const TRANSACTION_TIMEOUT_DURATION_SECS: u64 = 10 * 60;

    pub fn new() -> Self {
        Self {
            transactions: IndexMap::new(),
        }
    }

    /// Broadcast known transaction IDs to the connected peers.
    ///
    /// If the timeout period has passed for a transaction ID, it is broadcasted again.
    /// If the transaction has not been broadcasted, the transaction ID is broadcasted.
    pub fn on_tick<'a>(
        &mut self,
        connected_peers: impl Iterator<Item = &'a PeerId>,
    ) -> Vec<(PeerId, Vec<Txid>)> {
        // Remove timeout transactions.

        connected_peers
            .filter_map(|address| {
                let mut to_advertise = vec![];

                for (txid, info) in self.transactions.iter_mut() {
                    if !info.advertised.contains(address) {
                        to_advertise.push(*txid);
                        info.advertised.insert(*address);
                    }
                }

                if to_advertise.is_empty() {
                    None
                } else {
                    Some((*address, to_advertise))
                }
            })
            .collect()
    }

    pub fn get_transaction(&self, txid: &Txid) -> Option<Transaction> {
        self.transactions
            .get(txid)
            .map(|tx_info| tx_info.transaction.clone())
    }

    pub fn add_transaction(&mut self, raw_tx: &[u8]) {
        if let Ok(transaction) = deserialize::<Transaction>(raw_tx) {
            let txid = transaction.compute_txid();

            if self.transactions.len() == Self::MAX_TRANSACTIONS {
                self.transactions.shift_remove_index(0);
            }

            match self.transactions.entry(txid) {
                Entry::Occupied(_) => {
                    tracing::debug!("Tx {txid} already exists");
                }
                Entry::Vacant(entry) => {
                    entry.insert(TransactionInfo {
                        transaction,
                        advertised: HashSet::new(),
                        at: Instant::now(),
                    });
                    tracing::debug!("Added new tx {txid}");
                }
            }
        }
    }
}
