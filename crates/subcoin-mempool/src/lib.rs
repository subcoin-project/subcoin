//! # Bitcoin Mempool Overview
//!
//! 1. Transaction Validation.
//!     - Transactions are validated before being added to the mempool.
//!     - Validation includes checking transaction size, fees and script validity.
//! 2. Fee Management
//!     - Transactions are prioritized based on their fee rate.
//!     - The mempool may evict lower-fee transactions if it reaches its size limit.
//! 3. Ancestors and Descendants.
//!     - The mempool tracks transaction dependencies to ensure that transactions are minded the
//!       correct order.

mod arena;
mod coins_view;
mod error;
mod inner;
mod options;
mod policy;
// TODO: Re-enable when MockClient implements necessary traits
// #[cfg(test)]
// mod tests;
mod types;
mod validation;

pub use self::arena::{MemPoolArena, TxMemPoolEntry};
pub use self::coins_view::CoinsViewCache;
pub use self::error::MempoolError;
pub use self::inner::MemPoolInner;
pub use self::options::MemPoolOptions;
pub use self::types::{
    ConflictSet, EntryId, FeeRate, LockPoints, Package, PackageValidationResult, RemovalReason,
    ValidationResult,
};

use bitcoin::Transaction;
use bitcoin::hashes::Hash;
use sc_client_api::{AuxStore, HeaderBackend};
use sp_runtime::traits::Block as BlockT;
use std::collections::HashSet;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use subcoin_primitives::tx_pool::*;
use subcoin_primitives::{BackendExt, ClientExt};
use subcoin_utxo_storage::NativeUtxoStorage;

/// Thread-safe Bitcoin mempool.
///
/// Uses RwLock for interior mutability with the following lock hierarchy:
/// 1. MemPool::inner (RwLock)
/// 2. MemPool::coins_cache (RwLock)
///
/// **CRITICAL:** Always acquire locks in this order to avoid deadlocks.
pub struct MemPool<Block: BlockT, Client> {
    /// Configuration (immutable after creation).
    options: MemPoolOptions,

    /// Thread-safe inner state.
    inner: RwLock<MemPoolInner>,

    /// UTXO cache (separate lock to reduce contention).
    coins_cache: RwLock<CoinsViewCache>,

    /// Atomic counters (lockless).
    transactions_updated: AtomicU32,
    sequence_number: AtomicU64,

    /// Substrate client for chain state access.
    client: Arc<Client>,

    _phantom: PhantomData<Block>,
}

impl<Block, Client> MemPool<Block, Client>
where
    Block: BlockT,
    Client: HeaderBackend<Block> + AuxStore + Send + Sync,
{
    /// Create a new mempool with default options.
    pub fn new(client: Arc<Client>, utxo_storage: Arc<NativeUtxoStorage>) -> Self {
        Self::with_options(client, utxo_storage, MemPoolOptions::default())
    }

    /// Create a new mempool with custom options.
    pub fn with_options(
        client: Arc<Client>,
        utxo_storage: Arc<NativeUtxoStorage>,
        options: MemPoolOptions,
    ) -> Self {
        let coins_cache = CoinsViewCache::new(utxo_storage, 10_000);

        Self {
            options,
            inner: RwLock::new(MemPoolInner::new()),
            coins_cache: RwLock::new(coins_cache),
            transactions_updated: AtomicU32::new(0),
            sequence_number: AtomicU64::new(1),
            client,
            _phantom: PhantomData,
        }
    }

    /// Accept a single transaction into the mempool.
    ///
    /// **CRITICAL:** Holds write lock for entire ATMP flow to prevent TOCTOU races.
    pub fn accept_single_transaction(&self, tx: Transaction) -> Result<(), MempoolError> {
        // Acquire both locks for entire validation + commit (prevents TOCTOU)
        let mut inner = self.inner.write().expect("MemPool lock poisoned");
        let mut coins = self.coins_cache.write().expect("CoinsCache lock poisoned");

        // Get current chain state
        let info = self.client.info();
        let best_block = info.best_hash;
        let current_height: u32 = info
            .best_number
            .try_into()
            .unwrap_or_else(|_| panic!("Block number must fit into u32"));
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("Time went backwards")
            .as_secs() as i64;

        // Get current MTP from ClientExt
        let current_mtp =
            if let Some(bitcoin_block_hash) = self.client.bitcoin_block_hash_for(best_block) {
                self.client
                    .get_block_metadata(bitcoin_block_hash)
                    .map(|metadata| metadata.median_time_past)
                    .unwrap_or(current_time) // Fallback to current time if API fails
            } else {
                current_time // Fallback if block hash not found
            };

        // Create validation workspace
        let tx_arc = Arc::new(tx);
        let mut ws = validation::ValidationWorkspace::new(tx_arc);
        validation::pre_checks(
            &mut ws,
            &inner,
            &mut coins,
            &self.options,
            current_height,
            current_mtp,
        )?;

        // Stage 2: Check package limits (ancestors/descendants)
        validation::check_package_limits(&ws, &inner, &self.options)?;

        // Stage 3: PolicyScriptChecks (standard script validation)
        validation::check_inputs(&ws, &mut coins, validation::standard_script_verify_flags())?;

        // Stage 4: ConsensusScriptChecks (consensus script validation)
        validation::check_inputs(&ws, &mut coins, validation::mandatory_script_verify_flags())?;

        // Stage 5: Finalize - add to mempool
        let sequence = self.sequence_number.fetch_add(1, Ordering::SeqCst);
        // Get Bitcoin block hash for entry_block_hash
        let entry_block_hash = self
            .client
            .bitcoin_block_hash_for(best_block)
            .unwrap_or_else(bitcoin::BlockHash::all_zeros);
        let _entry_id = validation::finalize_tx(
            ws,
            &mut inner,
            &mut coins,
            current_height,
            current_time,
            current_mtp,
            entry_block_hash,
            sequence,
        )?;

        // Increment transactions_updated counter
        self.transactions_updated.fetch_add(1, Ordering::SeqCst);

        Ok(())
    }

    /// Get number of transactions in mempool.
    pub fn size(&self) -> usize {
        self.inner.read().expect("MemPool lock poisoned").size()
    }

    /// Get total size of all transactions in bytes.
    pub fn total_size(&self) -> u64 {
        self.inner
            .read()
            .expect("MemPool lock poisoned")
            .total_size()
    }

    /// Trim mempool to maximum size.
    pub fn trim_to_size(&self, max_size: u64) {
        self.inner
            .write()
            .expect("MemPool lock poisoned")
            .trim_to_size(max_size);
    }

    /// Expire old transactions.
    pub fn expire(&self, max_age_seconds: i64) {
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("Time went backwards")
            .as_secs() as i64;

        self.inner
            .write()
            .expect("MemPool lock poisoned")
            .expire(current_time, max_age_seconds);
    }

    /// Get mempool options.
    pub fn options(&self) -> &MemPoolOptions {
        &self.options
    }

    /// Remove transactions confirmed in a block.
    ///
    /// This is called after a new block is connected to remove transactions
    /// that were included in the block, as well as any conflicts.
    ///
    /// **Lock hierarchy**: Acquires inner write lock, then coins_cache write lock.
    pub fn remove_for_block(&self, confirmed_txs: &[Transaction]) -> Result<(), MempoolError> {
        let mut inner = self.inner.write().expect("MemPool lock poisoned");
        let mut coins = self.coins_cache.write().expect("CoinsCache lock poisoned");

        let mut to_remove = HashSet::new();

        // Phase 1: Collect all transactions to remove (confirmed + conflicts)
        for tx in confirmed_txs {
            let txid = tx.compute_txid();

            // Add the confirmed transaction itself
            if let Some(entry_id) = inner.arena.get_by_txid(&txid) {
                to_remove.insert(entry_id);
            }

            // Collect conflicts BEFORE any removal (map_next_tx will be mutated)
            let conflicts = Self::collect_conflicts(tx, &inner);
            to_remove.extend(conflicts);
        }

        // Phase 2: Expand to include all descendants
        let mut expanded = HashSet::new();
        for &entry_id in &to_remove {
            inner.calculate_descendants(entry_id, &mut expanded);
        }

        // Phase 3: Capture transaction data BEFORE removal (for coins cleanup)
        let mut removed_txs = Vec::with_capacity(expanded.len());
        for &entry_id in &expanded {
            if let Some(entry) = inner.arena.get(entry_id) {
                removed_txs.push(entry.tx.clone());
            }
        }

        // Phase 4: Remove from mempool
        inner.remove_staged(&expanded, false, RemovalReason::Block);

        // Phase 5: Clean up coins cache (remove mempool overlay entries)
        for tx in removed_txs {
            coins.remove_mempool_tx(&tx);
        }

        // Phase 6: Update coins cache for new block (clears base cache)
        coins.on_block_connected();

        // Increment transactions_updated counter
        self.transactions_updated
            .fetch_add(expanded.len() as u32, Ordering::SeqCst);

        Ok(())
    }

    /// Collect conflicting mempool transactions for a given transaction.
    ///
    /// **CRITICAL**: Must be called BEFORE any remove_staged() that would mutate map_next_tx.
    fn collect_conflicts(tx: &Transaction, inner: &MemPoolInner) -> HashSet<EntryId> {
        let mut conflicts = HashSet::new();

        for input in &tx.input {
            if let Some(conflicting_txid) = inner.get_conflict_tx(&input.previous_output) {
                if let Some(entry_id) = inner.arena.get_by_txid(&conflicting_txid) {
                    conflicts.insert(entry_id);
                }
            }
        }

        conflicts
    }

    /// Remove conflicts for a set of transactions.
    ///
    /// This is used when accepting a package of transactions to remove any
    /// existing mempool transactions that conflict with the package.
    pub fn remove_conflicts(&self, txs: &[Transaction]) -> Result<(), MempoolError> {
        let mut inner = self.inner.write().expect("MemPool lock poisoned");
        let mut coins = self.coins_cache.write().expect("CoinsCache lock poisoned");

        let mut to_remove = HashSet::new();

        // Collect all conflicts BEFORE any removal
        for tx in txs {
            let conflicts = Self::collect_conflicts(tx, &inner);
            to_remove.extend(conflicts);
        }

        // Expand to descendants
        let mut expanded = HashSet::new();
        for &entry_id in &to_remove {
            inner.calculate_descendants(entry_id, &mut expanded);
        }

        // Capture transaction data BEFORE removal (for coins cleanup)
        let mut removed_txs = Vec::with_capacity(expanded.len());
        for &entry_id in &expanded {
            if let Some(entry) = inner.arena.get(entry_id) {
                removed_txs.push(entry.tx.clone());
            }
        }

        // Remove from mempool
        inner.remove_staged(&expanded, false, RemovalReason::Conflict);

        // Clean up coins cache
        for tx in removed_txs {
            coins.remove_mempool_tx(&tx);
        }

        // Increment transactions_updated counter
        self.transactions_updated
            .fetch_add(expanded.len() as u32, Ordering::SeqCst);

        Ok(())
    }

    /// Remove transactions that are no longer valid after a reorg.
    ///
    /// Prunes transactions based on:
    /// - Height-based timelocks (nLockTime)
    /// - Coinbase maturity violations
    /// - Sequence-based timelocks (BIP68)
    /// - Max input block no longer on active chain
    ///
    /// **Lock hierarchy**: Acquires inner write lock, then coins_cache write lock.
    pub fn remove_for_reorg(&self, new_tip_height: u32) -> Result<(), MempoolError> {
        let mut inner = self.inner.write().expect("MemPool lock poisoned");
        let mut coins = self.coins_cache.write().expect("CoinsCache lock poisoned");

        // Get new tip's MTP for BIP68 validation
        let fallback_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("Time went backwards")
            .as_secs() as i64;

        let best_block = self.client.info().best_hash;
        let new_tip_mtp =
            if let Some(bitcoin_block_hash) = self.client.bitcoin_block_hash_for(best_block) {
                self.client
                    .get_block_metadata(bitcoin_block_hash)
                    .map(|metadata| metadata.median_time_past)
                    .unwrap_or(fallback_time)
            } else {
                fallback_time
            };

        let mut to_remove = HashSet::new();

        // Iterate all mempool entries and check validity at new tip
        for (entry_id, entry) in inner.arena.iter_by_entry_time() {
            let mut invalid = false;

            // Check height-based lock points (BIP68)
            if entry.lock_points.height > 0 && entry.lock_points.height > new_tip_height as i32 {
                invalid = true;
            }

            // Check time-based lock points (BIP68)
            if entry.lock_points.time > 0 && entry.lock_points.time > new_tip_mtp {
                invalid = true;
            }

            // Check if max_input_block is still on active chain (optional optimization)
            // If the block containing transaction inputs is no longer on the active chain,
            // the transaction may reference coins that don't exist anymore.
            if let Some(max_input_block) = entry.lock_points.max_input_block {
                let is_on_active_chain = self.client.is_block_on_active_chain(max_input_block);

                if !is_on_active_chain {
                    invalid = true;
                }
            }

            // Check coinbase maturity (100 blocks)
            if entry.spends_coinbase {
                // Re-validate coinbase maturity at new tip
                // This is conservative: if entry was created at height H and new tip is H-1,
                // the coinbase might not be mature anymore
                let min_required_height = entry.entry_height.saturating_add(100);
                if new_tip_height < min_required_height {
                    invalid = true;
                }
            }

            if invalid {
                to_remove.insert(entry_id);
            }
        }

        if to_remove.is_empty() {
            // Update coins cache even if no removals
            coins.on_block_connected();
            return Ok(());
        }

        // Expand to descendants
        let mut expanded = HashSet::new();
        for &entry_id in &to_remove {
            inner.calculate_descendants(entry_id, &mut expanded);
        }

        // Capture transaction data BEFORE removal
        let mut removed_txs = Vec::with_capacity(expanded.len());
        for &entry_id in &expanded {
            if let Some(entry) = inner.arena.get(entry_id) {
                removed_txs.push(entry.tx.clone());
            }
        }

        // Remove from mempool
        inner.remove_staged(&expanded, false, RemovalReason::Reorg);

        // Clean up coins cache
        for tx in removed_txs {
            coins.remove_mempool_tx(&tx);
        }

        // Update coins cache for new tip
        coins.on_block_connected();

        // Increment transactions_updated counter
        self.transactions_updated
            .fetch_add(expanded.len() as u32, Ordering::SeqCst);

        Ok(())
    }

    /// Accept a package of related transactions (CPFP support).
    ///
    /// All transactions are validated as a unit. If any fails, none are accepted.
    pub fn accept_package(
        &self,
        transactions: Vec<Transaction>,
    ) -> Result<PackageValidationResult, MempoolError> {
        if !self.options.enable_package_relay {
            return Err(MempoolError::PackageRelayDisabled);
        }

        // Acquire locks for entire package validation
        let mut inner = self.inner.write().expect("MemPool lock poisoned");
        let mut coins = self.coins_cache.write().expect("CoinsCache lock poisoned");

        // Get current state
        let info = self.client.info();
        let best_block = info.best_hash;
        let current_height: u32 = info
            .best_number
            .try_into()
            .unwrap_or_else(|_| panic!("Block number must fit into u32"));
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("Time went backwards")
            .as_secs() as i64;

        // Get current MTP for BIP68 validation
        let current_mtp =
            if let Some(bitcoin_block_hash) = self.client.bitcoin_block_hash_for(best_block) {
                self.client
                    .get_block_metadata(bitcoin_block_hash)
                    .map(|metadata| metadata.median_time_past)
                    .unwrap_or(current_time) // Fallback to current time if API fails
            } else {
                current_time // Fallback if block hash not found
            };

        // Convert to Arc (single allocation per tx)
        let arc_txs: Vec<_> = transactions.into_iter().map(Arc::new).collect();

        let package = Package {
            transactions: arc_txs,
        };

        // Validate and accept package (two-phase commit)
        let sequence_start = self.sequence_number.load(Ordering::SeqCst);
        let result = validation::validate_package(
            &package,
            &mut inner,
            &mut coins,
            &self.options,
            current_height,
            current_mtp,
            sequence_start,
        )?;

        // Update sequence counter
        self.sequence_number.store(
            sequence_start + result.accepted.len() as u64,
            Ordering::SeqCst,
        );

        // Update transactions counter
        self.transactions_updated
            .fetch_add(result.accepted.len() as u32, Ordering::SeqCst);

        Ok(result)
    }

    // --- Helper methods for TxPool trait ---

    /// Check if transaction exists in mempool.
    pub fn contains_txid(&self, txid: &bitcoin::Txid) -> bool {
        self.inner
            .read()
            .expect("MemPool lock poisoned")
            .arena
            .get_by_txid(txid)
            .is_some()
    }

    /// Get transaction from mempool if present.
    pub fn get_transaction(&self, txid: &bitcoin::Txid) -> Option<Arc<Transaction>> {
        self.inner
            .read()
            .expect("MemPool lock poisoned")
            .arena
            .get_by_txid(txid)
            .map(|entry_id| {
                self.inner
                    .read()
                    .expect("MemPool lock poisoned")
                    .arena
                    .get(entry_id)
                    .expect("Entry ID must be valid")
                    .tx
                    .clone()
            })
    }

    /// Get transactions pending broadcast with their fee rates.
    pub fn pending_broadcast_txs(&self) -> Vec<(bitcoin::Txid, u64)> {
        let inner = self.inner.read().expect("MemPool lock poisoned");
        inner
            .unbroadcast
            .iter()
            .filter_map(|txid| {
                inner.arena.get_by_txid(txid).and_then(|entry_id| {
                    inner.arena.get(entry_id).map(|entry| {
                        // Calculate fee rate: (fee * 1000) / vsize
                        let vsize = entry.tx_weight.to_wu().div_ceil(4); // Convert weight to vsize
                        let fee_rate = (entry.fee.to_sat() * 1000) / vsize;
                        (*txid, fee_rate)
                    })
                })
            })
            .collect()
    }

    /// Mark transactions as broadcast.
    pub fn mark_broadcast_txs(&self, txids: &[bitcoin::Txid]) {
        let mut inner = self.inner.write().expect("MemPool lock poisoned");
        for txid in txids {
            inner.unbroadcast.remove(txid);
        }
    }

    /// Iterate over all transaction IDs with their fee rates, sorted by mining priority.
    pub fn iter_txids_by_priority(&self) -> Vec<(bitcoin::Txid, u64)> {
        let inner = self.inner.read().expect("MemPool lock poisoned");
        // Already sorted by ancestor score (mining priority)
        inner
            .arena
            .iter_by_ancestor_score()
            .map(|(_, entry)| {
                let txid = entry.tx.compute_txid();
                // Calculate fee rate: (fee * 1000) / vsize
                let vsize = entry.tx_weight.to_wu().div_ceil(4);
                let fee_rate = (entry.fee.to_sat() * 1000) / vsize;
                (txid, fee_rate)
            })
            .collect()
    }

    /// Convert MempoolError to TxValidationResult for network integration.
    fn to_validation_result(
        &self,
        txid: bitcoin::Txid,
        result: Result<(), MempoolError>,
    ) -> subcoin_primitives::tx_pool::TxValidationResult {
        match result {
            Ok(()) => {
                // Transaction accepted - get its fee rate
                let inner = self.inner.read().expect("MemPool lock poisoned");
                let fee_rate = inner
                    .arena
                    .get_by_txid(&txid)
                    .and_then(|entry_id| {
                        inner.arena.get(entry_id).map(|entry| {
                            // Calculate fee rate: (fee * 1000) / vsize
                            let vsize = entry.tx_weight.to_wu().div_ceil(4);
                            (entry.fee.to_sat() * 1000) / vsize
                        })
                    })
                    .unwrap_or(0);

                TxValidationResult::Accepted { txid, fee_rate }
            }
            Err(err) => {
                let reason = match err {
                    // Soft rejections (don't penalize peer)
                    MempoolError::AlreadyInMempool => {
                        RejectionReason::Soft(SoftRejection::AlreadyInMempool)
                    }
                    MempoolError::MissingInputs { parents } => {
                        RejectionReason::Soft(SoftRejection::MissingInputs { parents })
                    }
                    MempoolError::FeeTooLow {
                        min_kvb,
                        actual_kvb,
                    } => RejectionReason::Soft(SoftRejection::FeeTooLow {
                        min_kvb,
                        actual_kvb,
                    }),
                    MempoolError::MempoolFull => RejectionReason::Soft(SoftRejection::MempoolFull),
                    MempoolError::TooManyAncestors(count) => {
                        RejectionReason::Soft(SoftRejection::TooManyAncestors(count))
                    }
                    MempoolError::TooManyDescendants(count) => {
                        RejectionReason::Soft(SoftRejection::TooManyDescendants(count))
                    }
                    MempoolError::TxConflict(msg) => {
                        RejectionReason::Soft(SoftRejection::TxConflict(msg))
                    }
                    MempoolError::NoConflictToReplace => {
                        RejectionReason::Soft(SoftRejection::NoConflictToReplace)
                    }
                    MempoolError::TxNotReplaceable => {
                        RejectionReason::Soft(SoftRejection::TxNotReplaceable)
                    }
                    MempoolError::TooManyReplacements(count) => {
                        RejectionReason::Soft(SoftRejection::TooManyReplacements(count))
                    }
                    MempoolError::NewUnconfirmedInput => {
                        RejectionReason::Soft(SoftRejection::NewUnconfirmedInput)
                    }
                    MempoolError::InsufficientFee(msg) => {
                        RejectionReason::Soft(SoftRejection::InsufficientFee(msg))
                    }
                    MempoolError::PackageTooLarge(count, max) => {
                        RejectionReason::Soft(SoftRejection::PackageTooLarge(count, max))
                    }
                    MempoolError::PackageSizeTooLarge(size) => {
                        RejectionReason::Soft(SoftRejection::PackageSizeTooLarge(size))
                    }
                    MempoolError::PackageCyclicDependencies => {
                        RejectionReason::Soft(SoftRejection::PackageCyclicDependencies)
                    }
                    MempoolError::PackageFeeTooLow(msg) => {
                        RejectionReason::Soft(SoftRejection::PackageFeeTooLow(msg))
                    }
                    MempoolError::PackageTxValidationFailed(txid, msg) => {
                        RejectionReason::Soft(SoftRejection::PackageTxValidationFailed(txid, msg))
                    }
                    MempoolError::PackageRelayDisabled => {
                        RejectionReason::Soft(SoftRejection::PackageRelayDisabled)
                    }

                    // Hard rejections (penalize peer)
                    MempoolError::Coinbase => RejectionReason::Hard(HardRejection::Coinbase),
                    MempoolError::NotStandard(msg) => {
                        RejectionReason::Hard(HardRejection::NotStandard(msg))
                    }
                    MempoolError::TxVersionNotStandard => {
                        RejectionReason::Hard(HardRejection::TxVersionNotStandard)
                    }
                    MempoolError::TxSizeTooSmall => {
                        RejectionReason::Hard(HardRejection::TxSizeTooSmall)
                    }
                    MempoolError::NonFinal => RejectionReason::Hard(HardRejection::NonFinal),
                    MempoolError::NonBIP68Final => {
                        RejectionReason::Hard(HardRejection::NonBIP68Final)
                    }
                    MempoolError::TooManySigops(count) => {
                        RejectionReason::Hard(HardRejection::TooManySigops(count))
                    }
                    MempoolError::NegativeFee => RejectionReason::Hard(HardRejection::NegativeFee),
                    MempoolError::FeeOverflow => RejectionReason::Hard(HardRejection::FeeOverflow),
                    MempoolError::InvalidFeeRate(msg) => {
                        RejectionReason::Hard(HardRejection::InvalidFeeRate(msg))
                    }
                    MempoolError::AncestorSizeTooLarge(size) => {
                        RejectionReason::Hard(HardRejection::AncestorSizeTooLarge(size))
                    }
                    MempoolError::DescendantSizeTooLarge(size) => {
                        RejectionReason::Hard(HardRejection::DescendantSizeTooLarge(size))
                    }
                    MempoolError::ScriptValidationFailed(msg) => {
                        RejectionReason::Hard(HardRejection::ScriptValidationFailed(msg))
                    }
                    MempoolError::TxError(err) => {
                        RejectionReason::Hard(HardRejection::TxError(err.to_string()))
                    }
                    MempoolError::RuntimeApi(msg) => {
                        RejectionReason::Hard(HardRejection::RuntimeApi(msg))
                    }
                    MempoolError::MissingConflict => {
                        // Internal error - treat as hard rejection
                        RejectionReason::Hard(HardRejection::RuntimeApi(
                            "Missing conflict transaction".to_string(),
                        ))
                    }
                };

                TxValidationResult::Rejected { txid, reason }
            }
        }
    }
}

// --- TxPool trait implementation ---

impl<Block, Client> subcoin_primitives::tx_pool::TxPool for MemPool<Block, Client>
where
    Block: BlockT + 'static,
    Client: HeaderBackend<Block> + AuxStore + Send + Sync + 'static,
{
    fn validate_transaction(
        &self,
        tx: Transaction,
    ) -> subcoin_primitives::tx_pool::TxValidationResult {
        let txid = tx.compute_txid();
        let result = self.accept_single_transaction(tx);
        self.to_validation_result(txid, result)
    }

    fn contains(&self, txid: &bitcoin::Txid) -> bool {
        self.contains_txid(txid)
    }

    fn get(&self, txid: &bitcoin::Txid) -> Option<Arc<Transaction>> {
        self.get_transaction(txid)
    }

    fn pending_broadcast(&self) -> Vec<(bitcoin::Txid, u64)> {
        self.pending_broadcast_txs()
    }

    fn mark_broadcast(&self, txids: &[bitcoin::Txid]) {
        self.mark_broadcast_txs(txids)
    }

    fn iter_txids(&self) -> Box<dyn Iterator<Item = (bitcoin::Txid, u64)> + Send> {
        Box::new(self.iter_txids_by_priority().into_iter())
    }

    fn info(&self) -> subcoin_primitives::tx_pool::TxPoolInfo {
        let inner = self.inner.read().expect("MemPool lock poisoned");
        subcoin_primitives::tx_pool::TxPoolInfo {
            size: inner.size(),
            bytes: inner.total_size(),
            usage: inner.total_size(), // Same as bytes for now
            min_fee_rate: self.options.min_relay_fee_rate().as_sat_per_kvb(),
        }
    }
}
