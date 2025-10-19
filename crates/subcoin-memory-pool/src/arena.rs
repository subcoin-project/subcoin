//! Arena-based mempool entry storage with multi-index support.
//!
//! The arena uses SlotMap for handle-based entry storage, avoiding reference cycles
//! and enabling safe mutation. Index keys are cached in entries to solve the
//! remove-before-mutate problem when updating BTreeSet indices.

use crate::types::{EntryId, LockPoints};
use bitcoin::{Transaction, Txid, Weight, Wtxid};
use slotmap::{DefaultKey, SlotMap};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

/// Comparable key for ancestor score index.
///
/// Transactions are prioritized by the minimum of their own feerate and
/// their ancestor feerate (ensures ancestors are mined first).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct AncestorScoreKey {
    /// Negated feerate for reverse sort (higher fee = lower value = earlier in BTreeSet).
    /// Uses fraction (fee * 1_000_000, size) for exact comparison without floats.
    pub neg_feerate_frac: (i64, i64),
    /// Tie-breaker (deterministic ordering).
    pub txid: Txid,
}

/// Comparable key for descendant score index.
///
/// Transactions are evicted based on the maximum of their own feerate and
/// their descendant feerate (evict lowest-paying clusters first).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct DescendantScoreKey {
    /// Negated feerate for reverse sort (higher fee = lower value = earlier in BTreeSet).
    pub neg_feerate_frac: (i64, i64),
    /// Secondary sort by entry time (older first for same feerate).
    pub time: i64,
}

/// Mempool entry with cached ancestor/descendant state.
///
/// **CRITICAL:** Index keys are cached to enable correct reindexing.
/// When mutating ancestor/descendant state, we must:
/// 1. Capture the old cached key
/// 2. Remove from indices using the old key
/// 3. Mutate the entry
/// 4. Recompute and cache the new key
/// 5. Reinsert using the new key
pub struct TxMemPoolEntry {
    /// Transaction data.
    pub tx: Arc<Transaction>,

    /// Base fee (without priority adjustments).
    pub fee: bitcoin::Amount,

    /// Modified fee (includes priority delta from prioritise_transaction).
    pub modified_fee: bitcoin::Amount,

    /// Cached transaction weight.
    pub tx_weight: Weight,

    /// Entry timestamp (seconds since epoch).
    pub time: i64,

    /// Block height when transaction entered mempool.
    pub entry_height: u32,

    /// Sequence number for replay protection.
    pub entry_sequence: u64,

    /// Whether this transaction spends a coinbase output.
    pub spends_coinbase: bool,

    /// Signature operation cost.
    pub sigop_cost: i64,

    /// Lock points for BIP68/BIP112 validation.
    pub lock_points: LockPoints,

    // === Mutable ancestor/descendant state ===
    /// Number of ancestors (including this tx).
    pub count_with_ancestors: u64,

    /// Total size of ancestors in virtual bytes (including this tx).
    pub size_with_ancestors: i64,

    /// Total fees of ancestors (including this tx).
    pub fees_with_ancestors: bitcoin::Amount,

    /// Total sigop cost of ancestors (including this tx).
    pub sigops_with_ancestors: i64,

    /// Number of descendants (including this tx).
    pub count_with_descendants: u64,

    /// Total size of descendants in virtual bytes (including this tx).
    pub size_with_descendants: i64,

    /// Total fees of descendants (including this tx).
    pub fees_with_descendants: bitcoin::Amount,

    // === Graph links (handles only, no direct references) ===
    /// Parent entries (in-mempool dependencies).
    pub parents: HashSet<EntryId>,

    /// Child entries (in-mempool dependents).
    pub children: HashSet<EntryId>,

    // === Cached index keys (updated atomically with state) ===
    /// Cached ancestor score key for efficient reindexing.
    pub cached_ancestor_key: AncestorScoreKey,

    /// Cached descendant score key for efficient reindexing.
    pub cached_descendant_key: DescendantScoreKey,

    /// Position in randomized transaction vector (for relay).
    pub idx_randomized: Option<usize>,
}

impl TxMemPoolEntry {
    /// Get transaction virtual size in bytes.
    pub fn vsize(&self) -> i64 {
        self.tx_weight.to_vbytes_ceil() as i64
    }

    /// Get transaction feerate (modified fee / vsize).
    pub fn feerate(&self) -> i64 {
        (self.modified_fee.to_sat() as i64 * 1_000_000) / self.vsize()
    }

    /// Get ancestor feerate.
    pub fn ancestor_feerate(&self) -> i64 {
        (self.fees_with_ancestors.to_sat() as i64 * 1_000_000) / self.size_with_ancestors
    }

    /// Get descendant feerate.
    pub fn descendant_feerate(&self) -> i64 {
        (self.fees_with_descendants.to_sat() as i64 * 1_000_000) / self.size_with_descendants
    }
}

/// Arena holding all mempool entries with multi-index support.
///
/// Provides O(1) lookups by txid/wtxid and sorted iteration by various criteria.
pub struct MemPoolArena {
    /// Primary storage: handle -> entry.
    entries: SlotMap<DefaultKey, TxMemPoolEntry>,

    // === Secondary indices (O(1) lookups) ===
    /// Index by transaction ID.
    by_txid: HashMap<Txid, EntryId>,

    /// Index by witness transaction ID.
    by_wtxid: HashMap<Wtxid, EntryId>,

    // === Sorted indices (for mining, eviction, iteration) ===
    /// Sorted by ancestor score (mining order: highest fee ancestors first).
    /// Key: (AncestorScoreKey, EntryId)
    by_ancestor_score: BTreeSet<(AncestorScoreKey, EntryId)>,

    /// Sorted by descendant score (eviction order: lowest fee descendants first).
    /// Key: (DescendantScoreKey, EntryId)
    by_descendant_score: BTreeSet<(DescendantScoreKey, EntryId)>,

    /// Sorted by entry time (for expiration).
    /// Key: (time, EntryId)
    by_entry_time: BTreeSet<(i64, EntryId)>,
}

impl MemPoolArena {
    /// Create a new empty arena.
    pub fn new() -> Self {
        Self {
            entries: SlotMap::new(),
            by_txid: HashMap::new(),
            by_wtxid: HashMap::new(),
            by_ancestor_score: BTreeSet::new(),
            by_descendant_score: BTreeSet::new(),
            by_entry_time: BTreeSet::new(),
        }
    }

    /// Insert a new entry into the arena.
    ///
    /// Returns the handle to the inserted entry.
    pub fn insert(&mut self, mut entry: TxMemPoolEntry) -> EntryId {
        // Compute and cache index keys
        let anc_key = Self::compute_ancestor_key(&entry);
        let desc_key = Self::compute_descendant_key(&entry);
        entry.cached_ancestor_key = anc_key;
        entry.cached_descendant_key = desc_key;

        let txid = entry.tx.compute_txid();
        let wtxid = entry.tx.compute_wtxid();
        let time = entry.time;

        // Insert into primary storage
        let id = EntryId(self.entries.insert(entry));

        // Update all indices
        self.by_txid.insert(txid, id);
        self.by_wtxid.insert(wtxid, id);
        self.by_ancestor_score.insert((anc_key, id));
        self.by_descendant_score.insert((desc_key, id));
        self.by_entry_time.insert((time, id));

        id
    }

    /// Update ancestor state and reindex.
    ///
    /// **CRITICAL:** Uses cached keys to remove old entries before mutation.
    pub fn update_ancestor_state(
        &mut self,
        id: EntryId,
        size_delta: i64,
        fee_delta: bitcoin::SignedAmount,
        count_delta: i64,
        sigops_delta: i64,
    ) {
        let entry = &self.entries[id.0];

        // CAPTURE old key BEFORE mutation
        let old_anc_key = entry.cached_ancestor_key;
        let old_desc_key = entry.cached_descendant_key;

        // Remove from indices using OLD keys
        self.by_ancestor_score.remove(&(old_anc_key, id));
        self.by_descendant_score.remove(&(old_desc_key, id));

        // Mutate entry
        let entry = &mut self.entries[id.0];
        entry.size_with_ancestors += size_delta;
        entry.fees_with_ancestors =
            (bitcoin::SignedAmount::from_sat(entry.fees_with_ancestors.to_sat() as i64)
                + fee_delta)
                .to_unsigned()
                .expect("Ancestor fees cannot go negative");
        entry.count_with_ancestors = (entry.count_with_ancestors as i64 + count_delta) as u64;
        entry.sigops_with_ancestors += sigops_delta;

        // Recompute keys
        let new_anc_key = Self::compute_ancestor_key(entry);
        let new_desc_key = Self::compute_descendant_key(entry);

        // Cache new keys in entry
        entry.cached_ancestor_key = new_anc_key;
        entry.cached_descendant_key = new_desc_key;

        // Reinsert with NEW keys
        self.by_ancestor_score.insert((new_anc_key, id));
        self.by_descendant_score.insert((new_desc_key, id));
    }

    /// Update descendant state and reindex.
    pub fn update_descendant_state(
        &mut self,
        id: EntryId,
        size_delta: i64,
        fee_delta: bitcoin::SignedAmount,
        count_delta: i64,
    ) {
        let entry = &self.entries[id.0];

        // CAPTURE old key
        let old_desc_key = entry.cached_descendant_key;

        // Remove using old key
        self.by_descendant_score.remove(&(old_desc_key, id));

        // Mutate
        let entry = &mut self.entries[id.0];
        entry.size_with_descendants += size_delta;
        entry.fees_with_descendants =
            (bitcoin::SignedAmount::from_sat(entry.fees_with_descendants.to_sat() as i64)
                + fee_delta)
                .to_unsigned()
                .expect("Descendant fees cannot go negative");
        entry.count_with_descendants = (entry.count_with_descendants as i64 + count_delta) as u64;

        // Recompute and cache
        let new_desc_key = Self::compute_descendant_key(entry);
        entry.cached_descendant_key = new_desc_key;

        // Reinsert
        self.by_descendant_score.insert((new_desc_key, id));
    }

    /// Update modified fee (priority adjustment) and reindex.
    pub fn update_modified_fee(&mut self, id: EntryId, new_modified_fee: bitcoin::Amount) {
        let entry = &self.entries[id.0];

        // CAPTURE old keys
        let old_anc_key = entry.cached_ancestor_key;
        let old_desc_key = entry.cached_descendant_key;

        // Remove from sorted indices
        self.by_ancestor_score.remove(&(old_anc_key, id));
        self.by_descendant_score.remove(&(old_desc_key, id));

        // Mutate
        let entry = &mut self.entries[id.0];
        let old_fee = entry.modified_fee;
        entry.modified_fee = new_modified_fee;

        // Update ancestor/descendant fees
        let fee_delta = bitcoin::Amount::from_sat(
            (new_modified_fee.to_sat() as i64 - old_fee.to_sat() as i64) as u64,
        );
        entry.fees_with_ancestors = bitcoin::Amount::from_sat(
            (entry.fees_with_ancestors.to_sat() as i64 + fee_delta.to_sat() as i64) as u64,
        );
        entry.fees_with_descendants = bitcoin::Amount::from_sat(
            (entry.fees_with_descendants.to_sat() as i64 + fee_delta.to_sat() as i64) as u64,
        );

        // Recompute and cache
        let new_anc_key = Self::compute_ancestor_key(entry);
        let new_desc_key = Self::compute_descendant_key(entry);
        entry.cached_ancestor_key = new_anc_key;
        entry.cached_descendant_key = new_desc_key;

        // Reinsert
        self.by_ancestor_score.insert((new_anc_key, id));
        self.by_descendant_score.insert((new_desc_key, id));
    }

    /// Compute ancestor score key for an entry.
    fn compute_ancestor_key(entry: &TxMemPoolEntry) -> AncestorScoreKey {
        let entry_feerate = entry.feerate();
        let ancestor_feerate = entry.ancestor_feerate();
        let min_feerate = std::cmp::min(entry_feerate, ancestor_feerate);

        AncestorScoreKey {
            neg_feerate_frac: (-min_feerate, entry.size_with_ancestors),
            txid: entry.tx.compute_txid(),
        }
    }

    /// Compute descendant score key for an entry.
    fn compute_descendant_key(entry: &TxMemPoolEntry) -> DescendantScoreKey {
        let entry_feerate = entry.feerate();
        let desc_feerate = entry.descendant_feerate();
        let max_feerate = std::cmp::max(entry_feerate, desc_feerate);

        DescendantScoreKey {
            neg_feerate_frac: (-max_feerate, entry.size_with_descendants),
            time: entry.time,
        }
    }

    /// Get entry by ID (immutable).
    pub fn get(&self, id: EntryId) -> Option<&TxMemPoolEntry> {
        self.entries.get(id.0)
    }

    /// Get entry by ID (mutable).
    pub fn get_mut(&mut self, id: EntryId) -> Option<&mut TxMemPoolEntry> {
        self.entries.get_mut(id.0)
    }

    /// Lookup entry ID by txid.
    pub fn get_by_txid(&self, txid: &Txid) -> Option<EntryId> {
        self.by_txid.get(txid).copied()
    }

    /// Lookup entry ID by wtxid.
    pub fn get_by_wtxid(&self, wtxid: &Wtxid) -> Option<EntryId> {
        self.by_wtxid.get(wtxid).copied()
    }

    /// Remove entry from arena.
    ///
    /// Returns the removed entry if it existed.
    pub fn remove(&mut self, id: EntryId) -> Option<TxMemPoolEntry> {
        let entry = self.entries.remove(id.0)?;

        // Remove from all indices
        self.by_txid.remove(&entry.tx.compute_txid());
        self.by_wtxid.remove(&entry.tx.compute_wtxid());
        self.by_ancestor_score
            .remove(&(entry.cached_ancestor_key, id));
        self.by_descendant_score
            .remove(&(entry.cached_descendant_key, id));
        self.by_entry_time.remove(&(entry.time, id));

        Some(entry)
    }

    /// Iterate entries sorted by ancestor score (highest fee first).
    ///
    /// This is the mining order.
    pub fn iter_by_ancestor_score(&self) -> impl Iterator<Item = (EntryId, &TxMemPoolEntry)> {
        self.by_ancestor_score
            .iter()
            .map(|(_, id)| (*id, &self.entries[id.0]))
    }

    /// Iterate entries sorted by descendant score (lowest fee first).
    ///
    /// This is the eviction order.
    pub fn iter_by_descendant_score(&self) -> impl Iterator<Item = (EntryId, &TxMemPoolEntry)> {
        self.by_descendant_score
            .iter()
            .map(|(_, id)| (*id, &self.entries[id.0]))
    }

    /// Iterate entries sorted by entry time (oldest first).
    pub fn iter_by_entry_time(&self) -> impl Iterator<Item = (EntryId, &TxMemPoolEntry)> {
        self.by_entry_time
            .iter()
            .map(|(_, id)| (*id, &self.entries[id.0]))
    }

    /// Get total number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if arena is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for MemPoolArena {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests will be added once we have the full entry construction logic
}
