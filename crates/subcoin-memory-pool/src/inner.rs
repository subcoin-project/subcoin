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
    /// - Optionally updates ancestor/descendant state for surviving children (RBF)
    pub fn remove_staged(
        &mut self,
        to_remove: &HashSet<EntryId>,
        update_descendants: bool,
        _reason: RemovalReason,
    ) {
        // Phase 1: Collect metadata for surviving children BEFORE any removal
        let surviving_children_metadata = if update_descendants {
            collect_surviving_children_metadata(&self.arena, to_remove)
        } else {
            Vec::new()
        };

        // Phase 2: Remove entries from arena
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
            }
        }

        // Phase 3: Update surviving children (AFTER removal)
        if update_descendants {
            for child_metadata in surviving_children_metadata {
                update_child_for_removed_ancestors(&mut self.arena, child_metadata);
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

/// Metadata for a child whose ancestors are being removed.
///
/// Captured BEFORE removal to preserve parent graph state.
struct SurvivingChildMetadata {
    child_id: EntryId,
    removed_ancestor_stats: AncestorStats,
}

/// Aggregated statistics for ancestors being removed.
#[derive(Default)]
struct AncestorStats {
    count: i64,
    size: i64,
    fees: bitcoin::SignedAmount,
    sigops: i64,
}

/// Collect metadata for surviving children before removal.
///
/// A "surviving child" is a transaction that:
/// - Has at least one parent in `to_remove`
/// - Has at least one parent NOT in `to_remove` (survives)
///
/// Returns metadata for each surviving child, capturing the stats of
/// ancestors that will be truly removed (not reachable through surviving parents).
fn collect_surviving_children_metadata(
    arena: &MemPoolArena,
    to_remove: &HashSet<EntryId>,
) -> Vec<SurvivingChildMetadata> {
    let mut result = Vec::new();

    // Find all children of transactions being removed
    let mut potential_survivors = HashSet::new();
    for &entry_id in to_remove {
        if let Some(entry) = arena.get(entry_id) {
            for &child_id in &entry.children {
                // Only consider children that are NOT being removed
                if !to_remove.contains(&child_id) {
                    potential_survivors.insert(child_id);
                }
            }
        }
    }

    // For each surviving child, calculate which ancestors are truly removed
    for child_id in potential_survivors {
        let removed_stats = calculate_truly_removed_ancestors(arena, child_id, to_remove);

        result.push(SurvivingChildMetadata {
            child_id,
            removed_ancestor_stats: removed_stats,
        });
    }

    result
}

/// Calculate statistics for ancestors that are truly being removed.
///
/// An ancestor is "truly removed" if:
/// - It's in the `to_remove` set
/// - It's NOT reachable through any surviving parent
///
/// This prevents double-counting shared ancestors.
fn calculate_truly_removed_ancestors(
    arena: &MemPoolArena,
    child_id: EntryId,
    to_remove: &HashSet<EntryId>,
) -> AncestorStats {
    // Step 1: Find all ancestors of this child
    let mut all_ancestors = HashSet::new();
    calculate_ancestors_recursive(arena, child_id, &mut all_ancestors);
    all_ancestors.remove(&child_id); // Exclude the child itself

    // Step 2: Find ancestors reachable through surviving parents
    let mut reachable_through_survivors = HashSet::new();
    if let Some(child_entry) = arena.get(child_id) {
        for &parent_id in &child_entry.parents {
            // If parent is NOT being removed, it's a surviving parent
            if !to_remove.contains(&parent_id) {
                // Add all ancestors of this surviving parent
                calculate_ancestors_recursive(arena, parent_id, &mut reachable_through_survivors);
            }
        }
    }

    // Step 3: Truly removed = (all ancestors ∩ to_remove) - reachable_through_survivors
    let mut stats = AncestorStats::default();

    for ancestor_id in all_ancestors {
        // Must be in removal set AND not reachable through survivors
        if to_remove.contains(&ancestor_id) && !reachable_through_survivors.contains(&ancestor_id) {
            if let Some(entry) = arena.get(ancestor_id) {
                stats.count += 1;
                stats.size += entry.vsize();
                stats.fees = bitcoin::SignedAmount::from_sat(
                    stats.fees.to_sat() + entry.modified_fee.to_sat() as i64,
                );
                stats.sigops += entry.sigop_cost;
            }
        }
    }

    stats
}

/// Recursively collect all ancestors (including the starting entry).
fn calculate_ancestors_recursive(
    arena: &MemPoolArena,
    entry_id: EntryId,
    ancestors: &mut HashSet<EntryId>,
) {
    if !ancestors.insert(entry_id) {
        return; // Already visited
    }

    if let Some(entry) = arena.get(entry_id) {
        for &parent_id in &entry.parents {
            calculate_ancestors_recursive(arena, parent_id, ancestors);
        }
    }
}

/// Update a surviving child after its ancestors have been removed.
///
/// Applies negative deltas to subtract the removed ancestors from the child's
/// cached ancestor state.
///
/// **CRITICAL**: Must be called AFTER entries are removed from arena.
fn update_child_for_removed_ancestors(arena: &mut MemPoolArena, metadata: SurvivingChildMetadata) {
    let stats = metadata.removed_ancestor_stats;

    // Apply negative deltas to the child
    arena.update_ancestor_state(
        metadata.child_id,
        -stats.size,
        -stats.fees,
        -stats.count,
        -stats.sigops,
    );

    // Also update all descendants of this child (they lose the same ancestors)
    let mut descendants = HashSet::new();
    if let Some(entry) = arena.get(metadata.child_id) {
        for &desc_id in &entry.children {
            calculate_descendants_recursive(arena, desc_id, &mut descendants);
        }
    }

    for desc_id in descendants {
        arena.update_ancestor_state(
            desc_id,
            -stats.size,
            -stats.fees,
            -stats.count,
            -stats.sigops,
        );
    }
}

/// Recursively collect all descendants (including the starting entry).
fn calculate_descendants_recursive(
    arena: &MemPoolArena,
    entry_id: EntryId,
    descendants: &mut HashSet<EntryId>,
) {
    if !descendants.insert(entry_id) {
        return; // Already visited
    }

    if let Some(entry) = arena.get(entry_id) {
        for &child_id in &entry.children {
            calculate_descendants_recursive(arena, child_id, descendants);
        }
    }
}

impl Default for MemPoolInner {
    fn default() -> Self {
        Self::new()
    }
}
