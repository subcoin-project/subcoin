//! Inner mempool state protected by RwLock.

use crate::arena::MemPoolArena;
use crate::types::{EntryId, RemovalReason};
use bitcoin::{Amount, OutPoint as COutPoint, Txid, Wtxid};
use std::collections::{HashMap, HashSet};

/// Inner mempool state (protected by RwLock in main MemPool).
pub struct MemPoolInner {
    /// Arena-based entry storage with multi-index support.
    pub(crate) arena: MemPoolArena,

    /// Track which outputs are spent by mempool transactions.
    /// Maps outpoint -> txid that spends it (for conflict detection).
    pub(crate) map_next_tx: HashMap<COutPoint, Txid>,

    /// Priority deltas (external to entries, applied via prioritise_transaction).
    /// Maps txid -> fee delta.
    pub(crate) map_deltas: HashMap<Txid, Amount>,

    /// Randomized list for transaction relay (getdata responses).
    /// Contains (wtxid, entry_id) pairs shuffled for privacy.
    pub(crate) txns_randomized: Vec<(Wtxid, EntryId)>,

    /// Transactions not yet broadcast to peers.
    pub(crate) unbroadcast: HashSet<Txid>,

    // === Statistics (updated on add/remove) ===
    /// Total size of all transactions in bytes.
    pub(crate) total_tx_size: u64,

    /// Total fees of all transactions.
    pub(crate) total_fee: Amount,

    /// Rolling minimum fee rate for mempool acceptance.
    /// Updated when mempool is trimmed to size.
    pub(crate) rolling_minimum_feerate: u64,

    /// Last time rolling fee was updated.
    pub(crate) last_rolling_fee_update: i64,
}

impl MemPoolInner {
    /// Create new empty mempool inner state.
    pub fn new() -> Self {
        Self {
            arena: MemPoolArena::new(),
            map_next_tx: HashMap::new(),
            map_deltas: HashMap::new(),
            txns_randomized: Vec::new(),
            unbroadcast: HashSet::new(),
            total_tx_size: 0,
            total_fee: Amount::ZERO,
            rolling_minimum_feerate: 0,
            last_rolling_fee_update: 0,
        }
    }

    /// Get entry by txid.
    pub fn get_entry(&self, txid: &Txid) -> Option<&crate::arena::TxMemPoolEntry> {
        let entry_id = self.arena.get_by_txid(txid)?;
        self.arena.get(entry_id)
    }

    /// Check if transaction exists in mempool by txid.
    pub fn contains_txid(&self, txid: &Txid) -> bool {
        self.arena.get_by_txid(txid).is_some()
    }

    /// Check if transaction exists in mempool by wtxid.
    pub fn contains_wtxid(&self, wtxid: &Wtxid) -> bool {
        self.arena.get_by_wtxid(wtxid).is_some()
    }

    /// Get transaction that spends the given outpoint (conflict detection).
    pub fn get_conflict_tx(&self, outpoint: &COutPoint) -> Option<Txid> {
        self.map_next_tx.get(outpoint).copied()
    }

    /// Calculate descendants of a transaction (recursively).
    ///
    /// Returns set of all descendant entry IDs (including the starting entry).
    pub fn calculate_descendants(&self, entry_id: EntryId, descendants: &mut HashSet<EntryId>) {
        if !descendants.insert(entry_id) {
            return; // Already visited
        }

        if let Some(entry) = self.arena.get(entry_id) {
            for &child_id in &entry.children {
                self.calculate_descendants(child_id, descendants);
            }
        }
    }

    /// Calculate ancestors of a transaction (recursively).
    ///
    /// Returns set of all ancestor entry IDs (including the starting entry).
    pub fn calculate_ancestors(&self, entry_id: EntryId, ancestors: &mut HashSet<EntryId>) {
        if !ancestors.insert(entry_id) {
            return; // Already visited
        }

        if let Some(entry) = self.arena.get(entry_id) {
            for &parent_id in &entry.parents {
                self.calculate_ancestors(parent_id, ancestors);
            }
        }
    }

    /// Remove transactions and update state.
    ///
    /// This is the core removal function that:
    /// - Removes entries from arena
    /// - Updates map_next_tx
    /// - Updates statistics
    /// - Does NOT update ancestor/descendant state (caller's responsibility)
    pub fn remove_staged(
        &mut self,
        to_remove: &HashSet<EntryId>,
        update_descendants: bool,
        _reason: RemovalReason,
    ) {
        for &entry_id in to_remove {
            if let Some(entry) = self.arena.remove(entry_id) {
                // Update statistics
                self.total_tx_size -= entry.tx_weight.to_wu();
                self.total_fee =
                    Amount::from_sat(self.total_fee.to_sat().saturating_sub(entry.fee.to_sat()));

                // Remove from map_next_tx
                for input in &entry.tx.input {
                    self.map_next_tx.remove(&input.previous_output);
                }

                // Remove from unbroadcast set
                self.unbroadcast.remove(&entry.tx.compute_txid());

                // NOTE: update_descendants parameter is currently unused because
                // all removal paths in Phase 4 remove entire descendant clusters.
                // No child ever survives its parent being removed, so there's no
                // ancestor state to update.
                //
                // TODO (Phase 5 - RBF/package handling): When implementing partial
                // removal where children can survive their parents, add Bitcoin Core's
                // UpdateForRemoveFromMempool logic here. This requires:
                // 1. Track the exact ancestor set being removed per surviving child
                // 2. Subtract each removed ancestor's stats exactly once
                // 3. Avoid double-counting ancestors shared between multiple parents
                //
                // Reference: Bitcoin Core's CTxMemPool::UpdateForRemoveFromMempool
                let _ = update_descendants;
            }
        }

        // Rebuild randomized transaction list
        self.rebuild_randomized_list();
    }

    /// Trim mempool to maximum size by evicting lowest-feerate transactions.
    pub fn trim_to_size(&mut self, max_size: u64) {
        while self.total_tx_size > max_size {
            // Get lowest feerate descendant cluster
            let Some((entry_id, _)) = self.arena.iter_by_descendant_score().next() else {
                break;
            };

            // Collect all descendants
            let mut to_remove = HashSet::new();
            self.calculate_descendants(entry_id, &mut to_remove);

            // Remove cluster
            self.remove_staged(&to_remove, false, RemovalReason::SizeLimit);
        }
    }

    /// Expire old transactions.
    pub fn expire(&mut self, current_time: i64, max_age_seconds: i64) {
        let cutoff_time = current_time - max_age_seconds;
        let mut to_remove = HashSet::new();

        // Find all transactions older than cutoff
        for (entry_id, entry) in self.arena.iter_by_entry_time() {
            if entry.time < cutoff_time {
                to_remove.insert(entry_id);
            } else {
                break; // Sorted by time, rest are newer
            }
        }

        if !to_remove.is_empty() {
            self.remove_staged(&to_remove, false, RemovalReason::Expiry);
        }
    }

    /// Rebuild the randomized transaction list for relay.
    fn rebuild_randomized_list(&mut self) {
        self.txns_randomized.clear();

        for (entry_id, entry) in self.arena.iter_by_ancestor_score() {
            self.txns_randomized
                .push((entry.tx.compute_wtxid(), entry_id));
        }

        // TODO: Shuffle for privacy (use a proper RNG)
        // For now, we keep the ancestor score order
    }

    /// Get total number of transactions.
    pub fn size(&self) -> usize {
        self.arena.len()
    }

    /// Get total transaction size in bytes.
    pub fn total_size(&self) -> u64 {
        self.total_tx_size
    }

    /// Get total fees.
    pub fn total_fees(&self) -> Amount {
        self.total_fee
    }
}

impl Default for MemPoolInner {
    fn default() -> Self {
        Self::new()
    }
}
