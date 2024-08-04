use crate::PeerId;
use bitcoin::{Transaction, Txid};
use indexmap::map::Entry;
use indexmap::IndexMap;
use std::collections::HashSet;
use std::time::{Duration, SystemTime};

const TRANSACTION_TIMEOUT_DURATION_SECS: u64 = 10 * 60;

#[derive(Debug)]
struct TransactionInfo {
    /// The actual transaction to be sent to the network.
    transaction: Transaction,
    /// Set of peers to which we advertised this transaction.
    ///
    /// Note that having a peer in this set doesn't guarantee the the peer actually
    /// received the transaction.
    advertised: HashSet<PeerId>,
    /// How long the transaction should be stored.
    ttl: SystemTime,
}

impl TransactionInfo {
    fn new(transaction: Transaction) -> Self {
        Self {
            transaction,
            advertised: HashSet::new(),
            ttl: SystemTime::now() + Duration::from_secs(TRANSACTION_TIMEOUT_DURATION_SECS),
        }
    }
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

    pub fn new() -> Self {
        Self {
            transactions: IndexMap::new(),
        }
    }

    /// Broadcast known transaction IDs to the connected peers.
    pub fn on_tick<'a>(
        &mut self,
        connected_peers: impl Iterator<Item = &'a PeerId>,
    ) -> Vec<(PeerId, Vec<Txid>)> {
        // Remove timeout transactions.
        let now = SystemTime::now();
        self.transactions.retain(|txid, info| {
            if info.ttl < now {
                tracing::debug!("Removing timeout transaction {txid}");
                false
            } else {
                true
            }
        });

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

    pub fn add_transaction(&mut self, transaction: Transaction) {
        let txid = transaction.compute_txid();

        if self.transactions.len() == Self::MAX_TRANSACTIONS {
            self.transactions.shift_remove_index(0);
        }

        match self.transactions.entry(txid) {
            Entry::Occupied(_) => {
                tracing::debug!("Tx {txid} already exists");
            }
            Entry::Vacant(entry) => {
                entry.insert(TransactionInfo::new(transaction));
                tracing::debug!("Added new tx {txid}");
            }
        }
    }
}
