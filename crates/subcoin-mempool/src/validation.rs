//! Transaction validation for mempool acceptance.
//!
//! Implements Bitcoin Core's AcceptToMemoryPool (ATMP) flow:
//! 1. PreChecks: Basic validity, standardness, input availability
//! 2. PolicyScriptChecks: Script validation with STANDARD_SCRIPT_VERIFY_FLAGS
//! 3. ConsensusScriptChecks: Script validation with consensus flags
//! 4. Finalize: Add to mempool, update indices

use crate::arena::TxMemPoolEntry;
use crate::coins_view::CoinsViewCache;
use crate::error::MempoolError;
use crate::inner::MemPoolInner;
use crate::options::MemPoolOptions;
use crate::policy::is_standard_tx;
use crate::types::{ConflictSet, EntryId, FeeRate, LockPoints};
use bitcoin::absolute::{LOCK_TIME_THRESHOLD, LockTime};
use bitcoin::hashes::Hash;
use bitcoin::{Transaction, Txid, Weight};
use sc_client_api::HeaderBackend;
use sp_api::ProvideRuntimeApi;
use sp_runtime::traits::Block as BlockT;
use std::collections::{HashMap, HashSet};
use std::convert::TryFrom;
use std::sync::Arc;
use subcoin_primitives::consensus::check_transaction_sanity;
use subcoin_script::{TransactionSignatureChecker, VerifyFlags, verify_script};

/// Workspace for validating a single transaction.
///
/// Holds intermediate state during validation to avoid recomputation.
pub struct ValidationWorkspace {
    /// Transaction being validated.
    pub tx: Arc<Transaction>,

    /// Base fee (sum of input values - sum of output values).
    pub base_fee: bitcoin::Amount,

    /// Modified fee (includes priority adjustments).
    pub modified_fee: bitcoin::Amount,

    /// Transaction weight.
    pub tx_weight: Weight,

    /// Virtual size in bytes.
    pub vsize: i64,

    /// Signature operation cost.
    pub sigop_cost: i64,

    /// Lock points for BIP68/BIP112.
    pub lock_points: LockPoints,

    /// Whether this transaction spends a coinbase output.
    pub spends_coinbase: bool,

    /// In-mempool ancestors (entry IDs).
    #[allow(dead_code)]
    pub ancestors: HashSet<EntryId>,

    /// Conflicting mempool transactions (txids).
    pub conflicts: HashSet<bitcoin::Txid>,

    /// Full conflict set for RBF (if any).
    pub conflict_set: Option<ConflictSet>,
}

impl ValidationWorkspace {
    /// Create new workspace for transaction validation.
    pub fn new(tx: Arc<Transaction>) -> Self {
        let tx_weight = tx.weight();
        let vsize = tx_weight.to_vbytes_ceil() as i64;

        Self {
            tx,
            base_fee: bitcoin::Amount::ZERO,
            modified_fee: bitcoin::Amount::ZERO,
            tx_weight,
            vsize,
            sigop_cost: 0,
            lock_points: LockPoints::default(),
            spends_coinbase: false,
            ancestors: HashSet::new(),
            conflicts: HashSet::new(),
            conflict_set: None,
        }
    }
}

/// Maximum standard sigop cost for a single transaction (80,000 weight units).
const MAX_STANDARD_TX_SIGOPS_COST: i64 = 80_000;

/// Pre-validation checks before script execution.
///
/// Corresponds to Bitcoin Core's `PreChecks()` function.
/// Validates:
/// - Transaction sanity (CheckTransaction)
/// - Not coinbase
/// - Standardness (if required)
/// - Finality (nLockTime; BIP68 deferred)
/// - Not already in mempool
/// - Input availability
/// - Fee requirements
pub fn pre_checks<Block, Client>(
    ws: &mut ValidationWorkspace,
    inner: &MemPoolInner,
    coins_cache: &mut CoinsViewCache<Block, Client>,
    options: &MemPoolOptions,
    current_height: u32,
    current_mtp: i64,
    _best_block: Block::Hash,
) -> Result<(), MempoolError>
where
    Block: BlockT,
    Client: ProvideRuntimeApi<Block> + HeaderBackend<Block> + Send + Sync,
    Client::Api: subcoin_runtime_primitives::SubcoinApi<Block>,
{
    let tx = &ws.tx;

    // 1. CheckTransaction (sanity checks)
    check_transaction_sanity(tx)?;

    // 2. Reject coinbase
    if tx.is_coinbase() {
        return Err(MempoolError::Coinbase);
    }

    // 3. Check standardness
    if options.require_standard {
        let dust_relay_feerate = FeeRate::from_sat_per_kvb(options.dust_relay_feerate);
        is_standard_tx(
            tx,
            options.max_datacarrier_bytes,
            options.permit_bare_multisig,
            dust_relay_feerate,
        )
        .map_err(|e| MempoolError::NotStandard(format!("{e:?}")))?;
    }

    // 4. Check if already in mempool (by wtxid)
    if inner.contains_wtxid(&tx.compute_wtxid()) {
        return Err(MempoolError::AlreadyInMempool);
    }

    // 5. Check if same txid exists (different witness)
    if inner.contains_txid(&tx.compute_txid()) {
        return Err(MempoolError::AlreadyInMempool);
    }

    // 6. Check for conflicts with mempool transactions (RBF handling)
    let mut has_conflicts = false;
    for input in &tx.input {
        if let Some(conflicting_txid) = inner.get_conflict_tx(&input.previous_output) {
            has_conflicts = true;
            ws.conflicts.insert(conflicting_txid);
        }
    }

    // If there are conflicts, check RBF policy
    if has_conflicts {
        if !options.enable_rbf {
            return Err(MempoolError::TxConflict(
                "conflicts with mempool transaction but RBF is disabled".to_string(),
            ));
        }

        // Validate RBF rules (BIP125)
        let conflict_set = check_rbf_policy(ws, inner, coins_cache, options)?;
        ws.conflict_set = Some(conflict_set);
    }

    // 7. Batch-prefetch all input coins
    let outpoints: Vec<_> = tx.input.iter().map(|txin| txin.previous_output).collect();
    coins_cache.ensure_coins(&outpoints)?;

    let mut prev_outputs: HashMap<_, _> = HashMap::with_capacity(outpoints.len());

    // 8. Check all inputs are available
    let mut missing = Vec::new();
    for outpoint in &outpoints {
        if !coins_cache.have_coin(outpoint) {
            missing.push(outpoint.txid);
        }
    }
    if !missing.is_empty() {
        missing.sort();
        missing.dedup();
        return Err(MempoolError::MissingInputs { parents: missing });
    }

    // 9. Calculate fees and check for negative fee
    let mut input_value = bitcoin::Amount::ZERO;
    let mut spends_coinbase = false;

    for outpoint in &outpoints {
        let coin = coins_cache
            .get_coin(outpoint)?
            .ok_or_else(|| MempoolError::MissingInputs {
                parents: vec![outpoint.txid],
            })?;

        input_value = input_value
            .checked_add(coin.output.value)
            .ok_or(MempoolError::FeeOverflow)?;

        prev_outputs.insert(*outpoint, coin.output.clone());

        if coin.is_coinbase {
            spends_coinbase = true;

            // Check coinbase maturity (100 blocks)
            let coin_age = current_height.saturating_sub(coin.height);
            if coin_age < 100 {
                return Err(MempoolError::NonFinal);
            }
        }
    }

    let output_value: bitcoin::Amount = tx.output.iter().map(|txout| txout.value).sum();

    if input_value < output_value {
        return Err(MempoolError::NegativeFee);
    }

    let base_fee = input_value
        .checked_sub(output_value)
        .ok_or(MempoolError::FeeOverflow)?;

    // 10. Check minimum relay fee
    let min_relay_fee_rate = options.min_relay_fee_rate();
    let min_fee = min_relay_fee_rate.get_fee(ws.vsize);
    if base_fee < min_fee {
        let actual_kvb = FeeRate::from_amount_and_vsize(base_fee, ws.vsize)
            .map_err(|e| MempoolError::InvalidFeeRate(e.to_string()))?
            .as_sat_per_kvb();
        let min_kvb = min_relay_fee_rate.as_sat_per_kvb();

        return Err(MempoolError::FeeTooLow {
            min_kvb,
            actual_kvb,
        });
    }

    // 11. Check finality (nLockTime only; BIP68 sequence locks deferred)
    let current_time_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("Time went backwards")
        .as_secs() as u32;

    if !is_final_tx(tx, current_height, current_time_secs) {
        return Err(MempoolError::NonFinal);
    }

    // 12. Calculate sigop cost
    let sigop_cost = ws
        .tx
        .total_sigop_cost(|outpoint| prev_outputs.get(outpoint).cloned());
    let sigop_cost = i64::try_from(sigop_cost).expect("sigop cost must fit into i64");

    if sigop_cost > MAX_STANDARD_TX_SIGOPS_COST {
        return Err(MempoolError::TooManySigops(sigop_cost));
    }

    // 13. Calculate BIP68 sequence lock points
    let lock_points = calculate_lock_points_at_tip(tx, coins_cache, current_height, current_mtp)?;

    // 14. Store computed values in workspace
    ws.base_fee = base_fee;
    ws.modified_fee = base_fee; // TODO: Apply priority deltas
    ws.spends_coinbase = spends_coinbase;
    ws.sigop_cost = sigop_cost;
    ws.lock_points = lock_points;

    Ok(())
}

/// Check ancestor/descendant limits before adding to mempool.
///
/// Ensures that adding this transaction won't violate:
/// - MAX_ANCESTORS (25)
/// - MAX_ANCESTOR_SIZE (101 KB)
/// - MAX_DESCENDANTS (25)
/// - MAX_DESCENDANT_SIZE (101 KB)
pub fn check_package_limits(
    ws: &ValidationWorkspace,
    inner: &MemPoolInner,
    options: &MemPoolOptions,
) -> Result<(), MempoolError> {
    // Find all in-mempool parents
    let mut ancestors = HashSet::new();

    for input in &ws.tx.input {
        if let Some(parent_id) = inner.arena.get_by_txid(&input.previous_output.txid) {
            // This input spends a mempool transaction
            inner.calculate_ancestors(parent_id, &mut ancestors);
        }
    }

    // Check ancestor limits
    let ancestor_count = ancestors.len() + 1; // +1 for this tx
    if ancestor_count > options.max_ancestors() {
        return Err(MempoolError::TooManyAncestors(ancestor_count));
    }

    // Calculate total ancestor size
    let mut ancestor_size = ws.vsize;
    for &ancestor_id in &ancestors {
        if let Some(entry) = inner.arena.get(ancestor_id) {
            ancestor_size += entry.vsize();
        }
    }

    if ancestor_size > options.max_ancestor_size() {
        return Err(MempoolError::AncestorSizeTooLarge(ancestor_size));
    }

    // Check descendant limits for each ancestor
    for &ancestor_id in &ancestors {
        let entry = inner
            .arena
            .get(ancestor_id)
            .expect("Ancestor entry must exist");

        // Adding this tx will add 1 descendant and ws.vsize to all ancestors
        let new_desc_count = entry.count_with_descendants + 1;
        let new_desc_size = entry.size_with_descendants + ws.vsize;

        if new_desc_count > options.max_descendants() {
            return Err(MempoolError::TooManyDescendants(new_desc_count as usize));
        }

        if new_desc_size > options.max_descendant_size() {
            return Err(MempoolError::DescendantSizeTooLarge(new_desc_size));
        }
    }

    Ok(())
}

/// Finalize transaction addition to mempool.
///
/// Creates TxMemPoolEntry and adds it to arena, updating ancestor/descendant state.
/// If conflict_set is present, removes conflicting transactions first (RBF).
#[allow(clippy::too_many_arguments)]
pub fn finalize_tx<Block, Client>(
    ws: ValidationWorkspace,
    inner: &mut MemPoolInner,
    coins_cache: &mut CoinsViewCache<Block, Client>,
    current_height: u32,
    current_time: i64,
    entry_block_mtp: i64,
    entry_block_hash: bitcoin::BlockHash,
    sequence: u64,
) -> Result<EntryId, MempoolError>
where
    Block: BlockT,
    Client: ProvideRuntimeApi<Block> + HeaderBackend<Block> + Send + Sync,
    Client::Api: subcoin_runtime_primitives::SubcoinApi<Block>,
{
    // Handle RBF: Remove conflicting transactions if present
    if let Some(conflict_set) = &ws.conflict_set {
        // Remove from mempool (with update_descendants = true for RBF)
        inner.remove_staged(
            &conflict_set.all_conflicts,
            true, // update_descendants = true for RBF
            crate::types::RemovalReason::Replaced,
        );

        // Clean up coins cache overlay for removed transactions
        for removed_tx in &conflict_set.removed_transactions {
            coins_cache.remove_mempool_tx(removed_tx);
        }
    }

    // Find in-mempool parents
    let mut parents = HashSet::new();
    let mut ancestors = HashSet::new();

    for input in &ws.tx.input {
        if let Some(parent_id) = inner.arena.get_by_txid(&input.previous_output.txid) {
            parents.insert(parent_id);
            inner.calculate_ancestors(parent_id, &mut ancestors);
        }
    }

    // Calculate initial ancestor state (includes this tx)
    let mut count_with_ancestors = 1;
    let mut size_with_ancestors = ws.vsize;
    let mut fees_with_ancestors = ws.modified_fee;
    let mut sigops_with_ancestors = ws.sigop_cost;

    for &ancestor_id in &ancestors {
        let entry = inner.arena.get(ancestor_id).expect("Ancestor must exist");
        count_with_ancestors += 1;
        size_with_ancestors += entry.vsize();
        fees_with_ancestors = fees_with_ancestors
            .checked_add(entry.modified_fee)
            .ok_or(MempoolError::FeeOverflow)?;
        sigops_with_ancestors += entry.sigop_cost;
    }

    // Create entry with initial state
    let entry = TxMemPoolEntry {
        tx: ws.tx.clone(),
        fee: ws.base_fee,
        modified_fee: ws.modified_fee,
        tx_weight: ws.tx_weight,
        time: current_time,
        entry_height: current_height,
        entry_block_mtp,
        entry_block_hash,
        entry_sequence: sequence,
        spends_coinbase: ws.spends_coinbase,
        sigop_cost: ws.sigop_cost,
        lock_points: ws.lock_points,
        count_with_ancestors,
        size_with_ancestors,
        fees_with_ancestors,
        sigops_with_ancestors,
        count_with_descendants: 1,
        size_with_descendants: ws.vsize,
        fees_with_descendants: ws.modified_fee,
        parents,
        children: HashSet::new(),
        cached_ancestor_key: crate::arena::AncestorScoreKey {
            neg_feerate_frac: (0, 1),
            txid: ws.tx.compute_txid(),
        }, // Will be recomputed by arena
        cached_descendant_key: crate::arena::DescendantScoreKey {
            neg_feerate_frac: (0, 1),
            time: current_time,
        }, // Will be recomputed by arena
        idx_randomized: None,
    };

    // Insert into arena (computes and caches index keys)
    let entry_id = inner.arena.insert(entry);

    // Update parent->child links
    for &parent_id in &ancestors {
        if let Some(parent_entry) = inner.arena.get_mut(parent_id) {
            parent_entry.children.insert(entry_id);
        }
    }

    // Update ancestor state for all ancestors
    let size_delta = ws.vsize;
    let fee_delta = bitcoin::SignedAmount::from_sat(ws.modified_fee.to_sat() as i64);
    let sigops_delta = ws.sigop_cost;

    for &ancestor_id in &ancestors {
        inner.arena.update_ancestor_state(
            ancestor_id,
            size_delta,
            fee_delta,
            1, // count_delta
            sigops_delta,
        );
    }

    // Update descendant state for all ancestors
    for &ancestor_id in &ancestors {
        inner.arena.update_descendant_state(
            ancestor_id,
            size_delta,
            fee_delta,
            1, // count_delta
        );
    }

    // Add to map_next_tx (mark outputs as spent)
    let txid = ws.tx.compute_txid();
    for input in &ws.tx.input {
        inner.map_next_tx.insert(input.previous_output, txid);
    }

    // Add to mempool overlay in coins cache
    coins_cache.add_mempool_coins(&ws.tx);

    // Update statistics
    inner.total_tx_size += ws.tx_weight.to_wu();
    inner.total_fee = inner
        .total_fee
        .checked_add(ws.base_fee)
        .ok_or(MempoolError::FeeOverflow)?;

    // Mark as unbroadcast
    inner.unbroadcast.insert(txid);

    Ok(entry_id)
}

/// Returns the mandatory consensus script verification flags.
pub fn mandatory_script_verify_flags() -> VerifyFlags {
    VerifyFlags::P2SH
        | VerifyFlags::DERSIG
        | VerifyFlags::NULLDUMMY
        | VerifyFlags::CHECKLOCKTIMEVERIFY
        | VerifyFlags::CHECKSEQUENCEVERIFY
        | VerifyFlags::WITNESS
        | VerifyFlags::TAPROOT
}

/// Returns the standard policy script verification flags.
pub fn standard_script_verify_flags() -> VerifyFlags {
    mandatory_script_verify_flags()
        | VerifyFlags::STRICTENC
        | VerifyFlags::LOW_S
        | VerifyFlags::SIGPUSHONLY
        | VerifyFlags::MINIMALDATA
        | VerifyFlags::MINIMALIF
        | VerifyFlags::NULLFAIL
        | VerifyFlags::WITNESS_PUBKEYTYPE
        | VerifyFlags::CLEANSTACK
        | VerifyFlags::DISCOURAGE_UPGRADABLE_NOPS
        | VerifyFlags::DISCOURAGE_UPGRADABLE_WITNESS_PROGRAM
        | VerifyFlags::DISCOURAGE_UPGRADABLE_TAPROOT_VERSION
        | VerifyFlags::DISCOURAGE_UPGRADABLE_PUBKEYTYPE
        | VerifyFlags::DISCOURAGE_OP_SUCCESS
        | VerifyFlags::CONST_SCRIPTCODE
}

/// Verify transaction scripts under the provided verification flags.
pub fn check_inputs<Block, Client>(
    ws: &ValidationWorkspace,
    coins_cache: &mut CoinsViewCache<Block, Client>,
    flags: VerifyFlags,
) -> Result<(), MempoolError>
where
    Block: BlockT,
    Client: ProvideRuntimeApi<Block> + HeaderBackend<Block> + Send + Sync,
    Client::Api: subcoin_runtime_primitives::SubcoinApi<Block>,
{
    let tx = &ws.tx;

    for (input_index, txin) in tx.input.iter().enumerate() {
        let coin = coins_cache
            .get_coin(&txin.previous_output)?
            .ok_or_else(|| MempoolError::MissingInputs {
                parents: vec![txin.previous_output.txid],
            })?;

        let mut checker =
            TransactionSignatureChecker::new(tx, input_index, coin.output.value.to_sat());

        verify_script(
            &txin.script_sig,
            &coin.output.script_pubkey,
            &txin.witness,
            &flags,
            &mut checker,
        )
        .map_err(|e| MempoolError::ScriptValidationFailed(format!("input {input_index}: {e}")))?;
    }

    Ok(())
}

/// Check if a replacement transaction satisfies BIP125 rules.
///
/// BIP125 Rules:
/// 1. All original transactions signal replaceability
/// 2. Replacement doesn't introduce new unconfirmed inputs
/// 3. Replacement pays higher absolute fee
/// 4. Replacement pays for its own bandwidth
/// 5. Replacement pays for replaced bandwidth
/// 6. No more than max_replacement_txs original transactions replaced
pub fn check_rbf_policy<Block, Client>(
    ws: &ValidationWorkspace,
    inner: &MemPoolInner,
    _coins_cache: &CoinsViewCache<Block, Client>,
    options: &MemPoolOptions,
) -> Result<ConflictSet, MempoolError>
where
    Block: BlockT,
    Client: ProvideRuntimeApi<Block> + HeaderBackend<Block> + Send + Sync,
    Client::Api: subcoin_runtime_primitives::SubcoinApi<Block>,
{
    let tx = &ws.tx;

    // Step 1: Find ALL direct conflicts (not just first one!)
    let mut direct_conflicts = HashSet::new();

    for input in &tx.input {
        if let Some(conflicting_txid) = inner.get_conflict_tx(&input.previous_output) {
            if let Some(entry_id) = inner.arena.get_by_txid(&conflicting_txid) {
                direct_conflicts.insert(entry_id);
            }
        }
    }

    if direct_conflicts.is_empty() {
        return Err(MempoolError::NoConflictToReplace);
    }

    // Step 2: Expand to include all descendants
    let mut all_conflicts = HashSet::new();
    for &conflict_id in &direct_conflicts {
        inner.calculate_descendants(conflict_id, &mut all_conflicts);
    }

    // Rule 6: Check replacement count limit EARLY
    if all_conflicts.len() > options.max_replacement_txs {
        return Err(MempoolError::TooManyReplacements(all_conflicts.len()));
    }

    // Rule 1: All direct conflicts must signal RBF
    for &conflict_id in &direct_conflicts {
        let conflict_entry = inner
            .arena
            .get(conflict_id)
            .ok_or(MempoolError::MissingConflict)?;

        if !conflict_entry.signals_rbf() {
            return Err(MempoolError::TxNotReplaceable);
        }
    }

    // Step 3: Calculate total fees and size of ALL replaced txs
    let mut replaced_fees = bitcoin::Amount::ZERO;
    let mut replaced_size: i64 = 0;
    let mut removed_transactions = Vec::with_capacity(all_conflicts.len());

    for &conflict_id in &all_conflicts {
        let entry = inner
            .arena
            .get(conflict_id)
            .ok_or(MempoolError::MissingConflict)?;

        replaced_fees = replaced_fees
            .checked_add(entry.modified_fee)
            .ok_or(MempoolError::FeeOverflow)?;
        replaced_size += entry.vsize();

        // CRITICAL: Capture Arc<Transaction> for coins cache cleanup
        removed_transactions.push(entry.tx.clone());
    }

    // Rule 2: Replacement doesn't introduce new unconfirmed inputs
    for input in &tx.input {
        if let Some(parent_id) = inner.arena.get_by_txid(&input.previous_output.txid) {
            if !all_conflicts.contains(&parent_id) {
                // Spending mempool tx that's not being replaced!
                return Err(MempoolError::NewUnconfirmedInput);
            }
        }
    }

    // Rule 3: Replacement pays higher absolute fee
    if ws.modified_fee <= replaced_fees {
        return Err(MempoolError::InsufficientFee(format!(
            "replacement fee {} <= replaced fees {replaced_fees}",
            ws.modified_fee
        )));
    }

    // Rule 4: Pays for own bandwidth (additional fee >= min_relay * new_size)
    let min_relay_rate = options.min_relay_fee_rate();
    let own_bandwidth_fee = min_relay_rate.get_fee(ws.vsize);

    let additional_fee_sat = ws.modified_fee.to_sat() as i64 - replaced_fees.to_sat() as i64;
    if additional_fee_sat < own_bandwidth_fee.to_sat() as i64 {
        return Err(MempoolError::InsufficientFee(format!(
            "doesn't pay for own bandwidth: additional {} < required {own_bandwidth_fee}",
            bitcoin::Amount::from_sat(additional_fee_sat as u64)
        )));
    }

    // Rule 5: Pays for replaced bandwidth (additional >= min_relay * replaced_size)
    let replaced_bandwidth_fee = min_relay_rate.get_fee(replaced_size);

    if additional_fee_sat < replaced_bandwidth_fee.to_sat() as i64 {
        return Err(MempoolError::InsufficientFee(format!(
            "doesn't pay for replaced bandwidth: additional {} < required {replaced_bandwidth_fee}",
            bitcoin::Amount::from_sat(additional_fee_sat as u64)
        )));
    }

    Ok(ConflictSet {
        direct_conflicts,
        all_conflicts,
        removed_transactions,
        replaced_fees,
        replaced_size,
    })
}

/// Check whether the transaction is final with respect to the current height and time.
fn is_final_tx(tx: &Transaction, height: u32, block_time: u32) -> bool {
    if tx.lock_time == LockTime::ZERO {
        return true;
    }

    let lock_time_limit = if tx.lock_time.to_consensus_u32() < LOCK_TIME_THRESHOLD {
        height
    } else {
        block_time
    };

    if tx.lock_time.to_consensus_u32() < lock_time_limit {
        return true;
    }

    tx.input.iter().all(|txin| txin.sequence.is_final())
}

/// Sort package transactions in topological order (parents before children).
///
/// Returns transactions sorted so all parents appear before their children.
/// Returns error if package contains cycles.
pub fn topological_sort_package(
    transactions: Vec<Arc<Transaction>>,
) -> Result<Vec<Arc<Transaction>>, MempoolError> {
    use std::collections::{HashMap, VecDeque};

    // Build txid -> tx mapping
    let tx_map: HashMap<Txid, Arc<Transaction>> = transactions
        .iter()
        .map(|tx| (tx.compute_txid(), tx.clone()))
        .collect();

    // Build dependency graph: txid -> set of in-package children
    let mut children: HashMap<Txid, Vec<Txid>> = HashMap::new();
    let mut in_degree: HashMap<Txid, usize> = HashMap::new();

    // Initialize in-degree for all txs
    for tx in &transactions {
        in_degree.insert(tx.compute_txid(), 0);
    }

    // Build edges
    for tx in &transactions {
        let txid = tx.compute_txid();
        for input in &tx.input {
            let parent_txid = input.previous_output.txid;
            // Only track in-package dependencies
            if tx_map.contains_key(&parent_txid) {
                children.entry(parent_txid).or_default().push(txid);
                *in_degree.get_mut(&txid).expect("txid must exist") += 1;
            }
        }
    }

    // Kahn's algorithm for topological sort
    let mut queue: VecDeque<Txid> = in_degree
        .iter()
        .filter(|&(_, &degree)| degree == 0)
        .map(|(&txid, _)| txid)
        .collect();

    let mut sorted = Vec::new();

    while let Some(txid) = queue.pop_front() {
        sorted.push(tx_map.get(&txid).expect("txid must exist").clone());

        if let Some(child_list) = children.get(&txid) {
            for &child_txid in child_list {
                let degree = in_degree.get_mut(&child_txid).expect("child must exist");
                *degree -= 1;
                if *degree == 0 {
                    queue.push_back(child_txid);
                }
            }
        }
    }

    // Check for cycles
    if sorted.len() != transactions.len() {
        return Err(MempoolError::PackageCyclicDependencies);
    }

    Ok(sorted)
}

/// Calculate total package fees and feerate.
///
/// Builds local map of in-package outputs to handle dependencies.
/// Returns (total_fee, total_vsize, package_feerate).
pub fn calculate_package_feerate<Block, Client>(
    sorted_txs: &[Arc<Transaction>],
    inner: &MemPoolInner,
    coins_cache: &mut CoinsViewCache<Block, Client>,
) -> Result<(bitcoin::Amount, i64, FeeRate), MempoolError>
where
    Block: BlockT,
    Client: ProvideRuntimeApi<Block> + HeaderBackend<Block> + Send + Sync,
    Client::Api: subcoin_runtime_primitives::SubcoinApi<Block>,
{
    use bitcoin::OutPoint;

    // Build map of in-package outputs: outpoint -> TxOut
    // This allows children to see parent outputs before they're in mempool
    let mut package_outputs: HashMap<OutPoint, bitcoin::TxOut> = HashMap::new();
    for tx in sorted_txs {
        let txid = tx.compute_txid();
        for (vout, output) in tx.output.iter().enumerate() {
            package_outputs.insert(
                OutPoint {
                    txid,
                    vout: vout as u32,
                },
                output.clone(),
            );
        }
    }

    let mut total_fee = bitcoin::Amount::ZERO;
    let mut total_vsize = 0i64;

    for tx in sorted_txs {
        // Calculate vsize from weight
        let vsize = tx.weight().to_vbytes_ceil() as i64;
        total_vsize = total_vsize
            .checked_add(vsize)
            .ok_or(MempoolError::FeeOverflow)?;

        // Calculate input value
        let mut input_value = bitcoin::Amount::ZERO;
        for input in &tx.input {
            let outpoint = &input.previous_output;

            // Check in this order:
            // 1. In-package outputs (earlier in topological order)
            // 2. Mempool outputs
            // 3. UTXO from coins_cache
            let output = if let Some(pkg_output) = package_outputs.get(outpoint) {
                pkg_output.clone()
            } else if let Some(parent_id) = inner.arena.get_by_txid(&outpoint.txid) {
                // Spends mempool transaction
                let parent =
                    inner
                        .arena
                        .get(parent_id)
                        .ok_or_else(|| MempoolError::MissingInputs {
                            parents: vec![outpoint.txid],
                        })?;
                parent
                    .tx
                    .output
                    .get(outpoint.vout as usize)
                    .ok_or_else(|| MempoolError::MissingInputs {
                        parents: vec![outpoint.txid],
                    })?
                    .clone()
            } else {
                // Spends UTXO
                let coin =
                    coins_cache
                        .get_coin(outpoint)?
                        .ok_or_else(|| MempoolError::MissingInputs {
                            parents: vec![outpoint.txid],
                        })?;
                coin.output
            };

            input_value = input_value
                .checked_add(output.value)
                .ok_or(MempoolError::FeeOverflow)?;
        }

        // Calculate output value
        let output_value: bitcoin::Amount = tx.output.iter().map(|out| out.value).sum();

        // Add to total fee
        let tx_fee = input_value
            .checked_sub(output_value)
            .ok_or(MempoolError::NegativeFee)?;
        total_fee = total_fee
            .checked_add(tx_fee)
            .ok_or(MempoolError::FeeOverflow)?;
    }

    // Guard against division by zero
    if total_vsize == 0 {
        return Err(MempoolError::PackageSizeTooLarge(0));
    }

    // Calculate package feerate: (total_fee_sats * 1000) / total_vsize
    let feerate_sat_kvb = total_fee
        .to_sat()
        .checked_mul(1000)
        .ok_or(MempoolError::FeeOverflow)?
        .checked_div(total_vsize as u64)
        .unwrap_or(0);

    let package_feerate = FeeRate::from_sat_per_kvb(feerate_sat_kvb);

    Ok((total_fee, total_vsize, package_feerate))
}

/// Pre-validate a single transaction in package context.
///
/// Runs all validation checks WITHOUT calling finalize_tx.
/// Returns ValidationWorkspace with all computed state.
///
/// IMPORTANT: Reuses Arc<Transaction> to avoid unnecessary cloning.
#[allow(clippy::too_many_arguments)]
pub fn pre_validate_package_tx<Block, Client>(
    tx: Arc<Transaction>,
    inner: &MemPoolInner,
    coins_cache: &mut CoinsViewCache<Block, Client>,
    options: &MemPoolOptions,
    current_height: u32,
    current_mtp: i64,
    best_block: Block::Hash,
    package_feerate: FeeRate,
) -> Result<ValidationWorkspace, MempoolError>
where
    Block: BlockT,
    Client: ProvideRuntimeApi<Block> + HeaderBackend<Block> + Send + Sync,
    Client::Api: subcoin_runtime_primitives::SubcoinApi<Block>,
{
    // Create workspace with Arc (no cloning)
    let mut ws = ValidationWorkspace::new(tx);

    // Run pre_checks (may relax feerate check for CPFP)
    match pre_checks(
        &mut ws,
        inner,
        coins_cache,
        options,
        current_height,
        current_mtp,
        best_block,
    ) {
        Ok(()) => {}
        Err(MempoolError::FeeTooLow {
            min_kvb,
            actual_kvb,
        }) => {
            // CPFP override: if package feerate meets minimum, allow low individual fee
            if package_feerate >= options.min_relay_fee_rate() {
                // Override fee check - package sponsors this transaction
                // Continue validation
            } else {
                return Err(MempoolError::FeeTooLow {
                    min_kvb,
                    actual_kvb,
                });
            }
        }
        Err(e) => return Err(e),
    }

    // Check package limits
    check_package_limits(&ws, inner, options)?;

    // Policy script checks
    check_inputs(&ws, coins_cache, standard_script_verify_flags())?;

    // Consensus script checks
    check_inputs(&ws, coins_cache, mandatory_script_verify_flags())?;

    // Return workspace with all validation passed
    Ok(ws)
}

/// Validate and accept a package of transactions (two-phase commit).
///
/// Phase 1: Pre-validate all transactions without modifying mempool
/// Phase 2: Finalize all transactions if all validations passed
#[allow(clippy::too_many_arguments)]
pub fn validate_package<Block, Client>(
    package: &crate::types::Package,
    inner: &mut MemPoolInner,
    coins_cache: &mut CoinsViewCache<Block, Client>,
    options: &MemPoolOptions,
    current_height: u32,
    best_block: Block::Hash,
    current_time: i64,
    sequence_start: u64,
) -> Result<crate::types::PackageValidationResult, MempoolError>
where
    Block: BlockT,
    Client: ProvideRuntimeApi<Block> + HeaderBackend<Block> + sc_client_api::AuxStore + Send + Sync,
    Client::Api: subcoin_runtime_primitives::SubcoinApi<Block>,
{
    // Step 1: Check package limits
    if package.transactions.len() > options.max_package_count {
        return Err(MempoolError::PackageTooLarge(
            package.transactions.len(),
            options.max_package_count,
        ));
    }

    // Calculate total vsize
    let total_vsize: u64 = package
        .transactions
        .iter()
        .map(|tx| tx.weight().to_vbytes_ceil())
        .sum();

    if total_vsize > options.max_package_vsize {
        return Err(MempoolError::PackageSizeTooLarge(total_vsize));
    }

    // Step 2: Topological sort (parents before children)
    let sorted_txs = topological_sort_package(package.transactions.clone())?;

    // Step 3: Calculate package feerate (with in-package outputs map)
    let (_total_fee, _total_vsize_i64, package_feerate) =
        calculate_package_feerate(&sorted_txs, inner, coins_cache)?;

    // Step 4: Check package meets minimum feerate
    if package_feerate < options.min_relay_fee_rate() {
        return Err(MempoolError::PackageFeeTooLow(format!(
            "package feerate {package_feerate:?} < min {:?}",
            options.min_relay_fee_rate()
        )));
    }

    // PHASE 1: Pre-validate all transactions (NO mempool modification)
    let mut validated_workspaces = Vec::new();
    // TODO (Phase 8): Get actual MTP from runtime API instead of using current_time
    let current_mtp = current_time;

    for tx in &sorted_txs {
        match pre_validate_package_tx(
            tx.clone(),
            inner,
            coins_cache,
            options,
            current_height,
            current_mtp,
            best_block,
            package_feerate,
        ) {
            Ok(ws) => {
                // Add to coins cache overlay for next tx in package
                // This makes in-package parent outputs visible to children
                coins_cache.add_mempool_coins(&ws.tx);
                validated_workspaces.push(ws);
            }
            Err(e) => {
                // Validation failed - rollback coins cache overlay
                for ws in &validated_workspaces {
                    coins_cache.remove_mempool_tx(&ws.tx);
                }
                return Err(MempoolError::PackageTxValidationFailed(
                    tx.compute_txid(),
                    format!("{e}"),
                ));
            }
        }
    }

    // PHASE 2: All validations passed - finalize all transactions
    let mut accepted = Vec::new();
    let mut sequence = sequence_start;

    // Get Bitcoin block hash for entry_block_hash
    use subcoin_primitives::BackendExt;
    let entry_block_hash = coins_cache
        .client()
        .clone()
        .bitcoin_block_hash_for(best_block)
        .unwrap_or_else(bitcoin::BlockHash::all_zeros);

    for ws in validated_workspaces {
        let txid = ws.tx.compute_txid();
        finalize_tx(
            ws,
            inner,
            coins_cache,
            current_height,
            current_time,
            current_mtp,
            entry_block_hash,
            sequence,
        )?;
        accepted.push(txid);
        sequence += 1;
    }

    Ok(crate::types::PackageValidationResult {
        accepted,
        package_feerate,
    })
}

/// BIP68 sequence lock constants
const SEQUENCE_LOCKTIME_DISABLE_FLAG: u32 = 1u32 << 31;
const SEQUENCE_LOCKTIME_TYPE_FLAG: u32 = 1u32 << 22;
const SEQUENCE_LOCKTIME_MASK: u32 = 0x0000ffff;
const SEQUENCE_LOCKTIME_GRANULARITY: i64 = 512; // 512 seconds

/// Calculate lock points for a transaction at the current chain tip (BIP68/BIP112).
///
/// This function computes the earliest block height and median time past (MTP) at which
/// a transaction becomes valid, based on its inputs' sequence values according to BIP68.
///
/// ## BIP68 Relative Lock-Time Encoding
///
/// Each input's nSequence value encodes:
/// - **Bit 31 (DISABLE_FLAG)**: If set, sequence locks are disabled for this input
/// - **Bit 22 (TYPE_FLAG)**: If clear = height-based, if set = time-based
/// - **Bits 0-15 (VALUE)**: The relative lock-time value
///
/// Height-based: Value is number of blocks since input's confirmation
/// Time-based: Value * 512 seconds since input's MTP
///
/// Coinbase inputs MUST have DISABLE_FLAG set (BIP68 rule).
///
/// ## Lock Points Calculation
///
/// For each input (unless DISABLE_FLAG is set):
/// 1. Lookup the input coin's block metadata (height, MTP)
/// 2. If height-based: `lock_height = coin_height + sequence_value + 1`
/// 3. If time-based: `lock_time = coin_mtp + (sequence_value * 512)`
/// 4. Track max lock height/time across all inputs
/// 5. Track highest input block hash (for reorg detection)
///
/// ## Returns
///
/// `LockPoints` with:
/// - `height`: Minimum block height for validity (-1 if no height locks)
/// - `time`: Minimum MTP for validity (0 if no time locks)
/// - `max_input_block`: Highest block containing any input (for reorg detection)
///
/// ## Errors
///
/// Returns `MempoolError::NonBIP68Final` if the transaction is not yet final according to
/// current chain tip.
pub fn calculate_lock_points_at_tip<Block, Client>(
    tx: &Transaction,
    coins_cache: &mut CoinsViewCache<Block, Client>,
    current_height: u32,
    current_mtp: i64,
) -> Result<LockPoints, MempoolError>
where
    Block: BlockT,
    Client: ProvideRuntimeApi<Block> + HeaderBackend<Block> + Send + Sync,
    Client::Api: subcoin_runtime_primitives::SubcoinApi<Block>,
{
    let mut lock_height: i32 = -1;
    let mut lock_time: i64 = 0;
    let max_input_block: Option<bitcoin::BlockHash> = None;
    let mut max_input_height: u32 = 0;

    for input in &tx.input {
        // Get coin for this input
        let coin = coins_cache
            .get_coin(&input.previous_output)?
            .ok_or_else(|| MempoolError::MissingInputs {
                parents: vec![input.previous_output.txid],
            })?;

        let sequence = input.sequence.0;

        // Check BIP68: coinbase inputs MUST have DISABLE_FLAG set
        if coin.is_coinbase && (sequence & SEQUENCE_LOCKTIME_DISABLE_FLAG) == 0 {
            return Err(MempoolError::NonBIP68Final);
        }

        // Skip if sequence locks disabled for this input
        if (sequence & SEQUENCE_LOCKTIME_DISABLE_FLAG) != 0 {
            continue;
        }

        // Track highest input block for reorg detection
        if coin.height > max_input_height {
            max_input_height = coin.height;
            // We'll need to get the block hash from the runtime
            // For now, we'll set this in a second pass or leave as None
            // TODO: Get block hash from runtime API
        }

        let masked_sequence = sequence & SEQUENCE_LOCKTIME_MASK;

        if (sequence & SEQUENCE_LOCKTIME_TYPE_FLAG) == 0 {
            // Height-based relative lock
            // Formula: lock_height = coin_height + masked_sequence + 1
            let coin_lock_height = coin
                .height
                .checked_add(masked_sequence)
                .and_then(|h| h.checked_add(1))
                .ok_or(MempoolError::NonBIP68Final)?;

            lock_height = lock_height.max(coin_lock_height as i32);
        } else {
            // Time-based relative lock (512-second granularity)
            // Formula: lock_time = coin_mtp + (masked_sequence * 512)
            let offset = (masked_sequence as i64)
                .checked_mul(SEQUENCE_LOCKTIME_GRANULARITY)
                .ok_or(MempoolError::NonBIP68Final)?;

            let coin_lock_time = coin
                .median_time_past
                .checked_add(offset)
                .ok_or(MempoolError::NonBIP68Final)?;

            lock_time = lock_time.max(coin_lock_time);
        }
    }

    // Check if transaction is final at current tip
    if lock_height > 0 && (current_height as i32) < lock_height {
        return Err(MempoolError::NonBIP68Final);
    }

    if lock_time > 0 && current_mtp < lock_time {
        return Err(MempoolError::NonBIP68Final);
    }

    Ok(LockPoints {
        height: lock_height,
        time: lock_time,
        max_input_block,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::absolute::LockTime;
    use bitcoin::{Amount, ScriptBuf, Sequence, TxIn, TxOut, transaction};

    fn create_test_tx(inputs: Vec<(Txid, u32)>, outputs: Vec<Amount>) -> Transaction {
        Transaction {
            version: transaction::Version::TWO,
            lock_time: LockTime::ZERO,
            input: inputs
                .into_iter()
                .map(|(txid, vout)| TxIn {
                    previous_output: bitcoin::OutPoint { txid, vout },
                    script_sig: ScriptBuf::new(),
                    sequence: Sequence::MAX,
                    witness: bitcoin::Witness::new(),
                })
                .collect(),
            output: outputs
                .into_iter()
                .map(|value| TxOut {
                    value,
                    script_pubkey: ScriptBuf::new(),
                })
                .collect(),
        }
    }

    #[test]
    fn test_topological_sort_chain() {
        let tx_a = Arc::new(create_test_tx(vec![], vec![Amount::from_sat(100_000)]));
        let txid_a = tx_a.compute_txid();
        let tx_b = Arc::new(create_test_tx(
            vec![(txid_a, 0)],
            vec![Amount::from_sat(90_000)],
        ));
        let txid_b = tx_b.compute_txid();
        let tx_c = Arc::new(create_test_tx(
            vec![(txid_b, 0)],
            vec![Amount::from_sat(80_000)],
        ));

        let sorted =
            topological_sort_package(vec![tx_c.clone(), tx_a.clone(), tx_b.clone()]).unwrap();

        assert_eq!(sorted[0].compute_txid(), txid_a);
        assert_eq!(sorted[1].compute_txid(), txid_b);
        assert_eq!(sorted[2].compute_txid(), tx_c.compute_txid());
    }

    #[test]
    fn test_topological_sort_cpfp() {
        let tx_a = Arc::new(create_test_tx(vec![], vec![Amount::from_sat(100_000)]));
        let tx_b = Arc::new(create_test_tx(vec![], vec![Amount::from_sat(50_000)]));
        let txid_a = tx_a.compute_txid();
        let txid_b = tx_b.compute_txid();
        let tx_c = Arc::new(create_test_tx(
            vec![(txid_a, 0), (txid_b, 0)],
            vec![Amount::from_sat(140_000)],
        ));
        let txid_c = tx_c.compute_txid();

        let sorted =
            topological_sort_package(vec![tx_c.clone(), tx_b.clone(), tx_a.clone()]).unwrap();

        let pos: std::collections::HashMap<_, _> = sorted
            .iter()
            .enumerate()
            .map(|(i, tx)| (tx.compute_txid(), i))
            .collect();

        assert!(pos[&txid_a] < pos[&txid_c]);
        assert!(pos[&txid_b] < pos[&txid_c]);
    }

    // Note: Actual cycle detection test would require creating transactions
    // with circular dependencies, which is cryptographically impossible in Bitcoin
    // (you can't know the txid before creating the transaction).
    // The cycle detection code path is exercised indirectly through chain tests.

    #[test]
    fn test_topological_sort_empty() {
        let sorted = topological_sort_package(vec![]).unwrap();
        assert_eq!(sorted.len(), 0);
    }

    #[test]
    fn test_topological_sort_single() {
        let tx = Arc::new(create_test_tx(vec![], vec![Amount::from_sat(100_000)]));
        let sorted = topological_sort_package(vec![tx.clone()]).unwrap();
        assert_eq!(sorted.len(), 1);
        assert_eq!(sorted[0].compute_txid(), tx.compute_txid());
    }

    // TODO: BIP68 Sequence Lock Tests - Enable once MockClient ApiExt is implemented
    // The test infrastructure is ready but requires ApiExt<Block> implementation
    // which is complex. These tests validate calculate_lock_points_at_tip() comprehensively.
    /*
    #[cfg(test)]
    mod bip68_tests {
        use super::*;
        use crate::coins_view::CoinsViewCache;
        use bitcoin::hashes::Hash;
        use bitcoin::{OutPoint, ScriptBuf, TxOut};
        use sc_client_api::HeaderBackend;
        use sp_api::ProvideRuntimeApi;
        use sp_runtime::testing::{Block as TestBlock, Header, TestXt};
        use std::collections::HashMap;
        use std::sync::{Arc, RwLock};
        use subcoin_primitives::PoolCoin;
        // TODO: Fix BIP68 test mocks to use new API structure

        type TestBlockType = TestBlock<TestXt<(), ()>>;

        /// Mock client for BIP68 testing
        struct MockBIP68Client {
            utxos: RwLock<HashMap<OutPoint, PoolCoin>>,
            best_block: RwLock<<TestBlockType as sp_runtime::traits::Block>::Hash>,
            best_number: RwLock<u64>,
        }

        impl MockBIP68Client {
            fn new() -> Self {
                Self {
                    utxos: RwLock::new(HashMap::new()),
                    best_block: RwLock::new(Default::default()),
                    best_number: RwLock::new(100),
                }
            }

            fn add_utxo(&self, outpoint: OutPoint, coin: PoolCoin) {
                self.utxos.write().unwrap().insert(outpoint, coin);
            }
        }

        impl HeaderBackend<TestBlockType> for MockBIP68Client {
            fn header(
                &self,
                _hash: <TestBlockType as sp_runtime::traits::Block>::Hash,
            ) -> sp_blockchain::Result<Option<Header>> {
                Ok(Some(Header::new_from_number(
                    *self.best_number.read().unwrap(),
                )))
            }

            fn info(&self) -> sc_client_api::blockchain::Info<TestBlockType> {
                sc_client_api::blockchain::Info {
                    best_hash: *self.best_block.read().unwrap(),
                    best_number: *self.best_number.read().unwrap(),
                    finalized_hash: Default::default(),
                    finalized_number: 0,
                    genesis_hash: Default::default(),
                    number_leaves: 1,
                    finalized_state: None,
                    block_gap: None,
                }
            }

            fn status(
                &self,
                _hash: <TestBlockType as sp_runtime::traits::Block>::Hash,
            ) -> sp_blockchain::Result<sc_client_api::blockchain::BlockStatus> {
                Ok(sc_client_api::blockchain::BlockStatus::InChain)
            }

            fn number(
                &self,
                _hash: <TestBlockType as sp_runtime::traits::Block>::Hash,
            ) -> sp_blockchain::Result<Option<u64>> {
                Ok(Some(*self.best_number.read().unwrap()))
            }

            fn hash(
                &self,
                _number: u64,
            ) -> sp_blockchain::Result<Option<<TestBlockType as sp_runtime::traits::Block>::Hash>>
            {
                Ok(Some(*self.best_block.read().unwrap()))
            }
        }

        impl ProvideRuntimeApi<TestBlockType> for MockBIP68Client {
            type Api = Self;

            fn runtime_api(&self) -> sp_api::ApiRef<Self::Api> {
                unimplemented!()
            }
        }

        // TODO: Implement mock using subcoin_runtime_primitives::SubcoinApi and subcoin_primitives::ClientExt

        fn create_coin(height: u32, mtp: i64, is_coinbase: bool) -> PoolCoin {
            PoolCoin {
                output: TxOut {
                    value: Amount::from_sat(100_000),
                    script_pubkey: ScriptBuf::new(),
                },
                height,
                is_coinbase,
                median_time_past: mtp,
            }
        }

        fn create_tx_with_sequence(sequence: u32) -> Transaction {
            let prev_txid = Txid::all_zeros();
            Transaction {
                version: transaction::Version::TWO,
                lock_time: LockTime::ZERO,
                input: vec![TxIn {
                    previous_output: OutPoint {
                        txid: prev_txid,
                        vout: 0,
                    },
                    script_sig: ScriptBuf::new(),
                    sequence: Sequence(sequence),
                    witness: bitcoin::Witness::new(),
                }],
                output: vec![TxOut {
                    value: Amount::from_sat(50_000),
                    script_pubkey: ScriptBuf::new(),
                }],
            }
        }

        #[test]
        fn test_height_based_relative_lock() {
            let client = Arc::new(MockBIP68Client::new());
            let mut coins_cache = CoinsViewCache::new(client.clone(), 1000);

            // Create a coin at height 50 with MTP 500000
            let prev_txid = Txid::all_zeros();
            let outpoint = OutPoint {
                txid: prev_txid,
                vout: 0,
            };
            client.add_utxo(outpoint, create_coin(50, 500000, false));

            // Create transaction with height-based lock: 10 blocks
            // Sequence value: 10 (no type flag = height-based)
            let tx = create_tx_with_sequence(10);

            let current_height = 100;
            let current_mtp = 1000000;

            let result =
                calculate_lock_points_at_tip(&tx, &mut coins_cache, current_height, current_mtp);

            assert!(result.is_ok());
            let lock_points = result.unwrap();
            // Lock height = coin_height (50) + sequence (10) + 1 = 61
            assert_eq!(lock_points.height, 61);
            assert_eq!(lock_points.time, 0);
        }

        #[test]
        fn test_time_based_relative_lock() {
            let client = Arc::new(MockBIP68Client::new());
            let mut coins_cache = CoinsViewCache::new(client.clone(), 1000);

            // Create a coin at height 50 with MTP 500000
            let prev_txid = Txid::all_zeros();
            let outpoint = OutPoint {
                txid: prev_txid,
                vout: 0,
            };
            client.add_utxo(outpoint, create_coin(50, 500000, false));

            // Create transaction with time-based lock: 10 * 512 seconds
            // Sequence value: 10 | TYPE_FLAG (0x00400000)
            const TYPE_FLAG: u32 = 0x00400000;
            let tx = create_tx_with_sequence(10 | TYPE_FLAG);

            let current_height = 100;
            let current_mtp = 1000000;

            let result =
                calculate_lock_points_at_tip(&tx, &mut coins_cache, current_height, current_mtp);

            assert!(result.is_ok());
            let lock_points = result.unwrap();
            assert_eq!(lock_points.height, -1);
            // Lock time = coin_mtp (500000) + (10 * 512) = 505120
            assert_eq!(lock_points.time, 505120);
        }

        #[test]
        fn test_disabled_sequence_lock() {
            let client = Arc::new(MockBIP68Client::new());
            let mut coins_cache = CoinsViewCache::new(client.clone(), 1000);

            let prev_txid = Txid::all_zeros();
            let outpoint = OutPoint {
                txid: prev_txid,
                vout: 0,
            };
            client.add_utxo(outpoint, create_coin(50, 500000, false));

            // Sequence with DISABLE_FLAG set (0x80000000)
            const DISABLE_FLAG: u32 = 0x80000000;
            let tx = create_tx_with_sequence(10 | DISABLE_FLAG);

            let current_height = 100;
            let current_mtp = 1000000;

            let result =
                calculate_lock_points_at_tip(&tx, &mut coins_cache, current_height, current_mtp);

            assert!(result.is_ok());
            let lock_points = result.unwrap();
            // Disabled locks should not contribute to lock points
            assert_eq!(lock_points.height, -1);
            assert_eq!(lock_points.time, 0);
        }

        #[test]
        fn test_coinbase_must_have_disable_flag() {
            let client = Arc::new(MockBIP68Client::new());
            let mut coins_cache = CoinsViewCache::new(client.clone(), 1000);

            let prev_txid = Txid::all_zeros();
            let outpoint = OutPoint {
                txid: prev_txid,
                vout: 0,
            };
            // Coinbase coin
            client.add_utxo(outpoint, create_coin(50, 500000, true));

            // Sequence WITHOUT disable flag - this should fail for coinbase
            let tx = create_tx_with_sequence(10);

            let current_height = 100;
            let current_mtp = 1000000;

            let result =
                calculate_lock_points_at_tip(&tx, &mut coins_cache, current_height, current_mtp);

            assert!(result.is_err());
            assert!(matches!(result, Err(MempoolError::NonBIP68Final)));
        }

        #[test]
        fn test_coinbase_with_disable_flag_ok() {
            let client = Arc::new(MockBIP68Client::new());
            let mut coins_cache = CoinsViewCache::new(client.clone(), 1000);

            let prev_txid = Txid::all_zeros();
            let outpoint = OutPoint {
                txid: prev_txid,
                vout: 0,
            };
            // Coinbase coin
            client.add_utxo(outpoint, create_coin(50, 500000, true));

            // Sequence with DISABLE_FLAG - this should succeed
            const DISABLE_FLAG: u32 = 0x80000000;
            let tx = create_tx_with_sequence(DISABLE_FLAG);

            let current_height = 100;
            let current_mtp = 1000000;

            let result =
                calculate_lock_points_at_tip(&tx, &mut coins_cache, current_height, current_mtp);

            assert!(result.is_ok());
        }

        #[test]
        fn test_not_final_at_current_height() {
            let client = Arc::new(MockBIP68Client::new());
            let mut coins_cache = CoinsViewCache::new(client.clone(), 1000);

            let prev_txid = Txid::all_zeros();
            let outpoint = OutPoint {
                txid: prev_txid,
                vout: 0,
            };
            client.add_utxo(outpoint, create_coin(50, 500000, false));

            // Require lock at height 100 (50 + 49 + 1)
            let tx = create_tx_with_sequence(49);

            // Current height is only 80 - should fail
            let current_height = 80;
            let current_mtp = 1000000;

            let result =
                calculate_lock_points_at_tip(&tx, &mut coins_cache, current_height, current_mtp);

            assert!(result.is_err());
            assert!(matches!(result, Err(MempoolError::NonBIP68Final)));
        }

        #[test]
        fn test_not_final_at_current_time() {
            let client = Arc::new(MockBIP68Client::new());
            let mut coins_cache = CoinsViewCache::new(client.clone(), 1000);

            let prev_txid = Txid::all_zeros();
            let outpoint = OutPoint {
                txid: prev_txid,
                vout: 0,
            };
            client.add_utxo(outpoint, create_coin(50, 500000, false));

            // Time-based lock: 1000 * 512 = 512000 seconds
            // Lock time would be: 500000 + 512000 = 1012000
            const TYPE_FLAG: u32 = 0x00400000;
            let tx = create_tx_with_sequence(1000 | TYPE_FLAG);

            let current_height = 100;
            // Current MTP is only 600000 - should fail
            let current_mtp = 600000;

            let result =
                calculate_lock_points_at_tip(&tx, &mut coins_cache, current_height, current_mtp);

            assert!(result.is_err());
            assert!(matches!(result, Err(MempoolError::NonBIP68Final)));
        }

        #[test]
        fn test_mixed_height_and_time_locks() {
            let client = Arc::new(MockBIP68Client::new());
            let mut coins_cache = CoinsViewCache::new(client.clone(), 1000);

            // Input 1: height-based lock
            let txid1 = Txid::from_byte_array([1u8; 32]);
            let outpoint1 = OutPoint {
                txid: txid1,
                vout: 0,
            };
            client.add_utxo(outpoint1, create_coin(50, 500000, false));

            // Input 2: time-based lock
            let txid2 = Txid::from_byte_array([2u8; 32]);
            let outpoint2 = OutPoint {
                txid: txid2,
                vout: 0,
            };
            client.add_utxo(outpoint2, create_coin(60, 600000, false));

            const TYPE_FLAG: u32 = 0x00400000;

            // Transaction with two inputs: one height-based, one time-based
            let tx = Transaction {
                version: transaction::Version::TWO,
                lock_time: LockTime::ZERO,
                input: vec![
                    TxIn {
                        previous_output: outpoint1,
                        script_sig: ScriptBuf::new(),
                        sequence: Sequence(10), // Height-based: 10 blocks
                        witness: bitcoin::Witness::new(),
                    },
                    TxIn {
                        previous_output: outpoint2,
                        script_sig: ScriptBuf::new(),
                        sequence: Sequence(20 | TYPE_FLAG), // Time-based: 20 * 512 seconds
                        witness: bitcoin::Witness::new(),
                    },
                ],
                output: vec![TxOut {
                    value: Amount::from_sat(50_000),
                    script_pubkey: ScriptBuf::new(),
                }],
            };

            let current_height = 100;
            let current_mtp = 1000000;

            let result =
                calculate_lock_points_at_tip(&tx, &mut coins_cache, current_height, current_mtp);

            assert!(result.is_ok());
            let lock_points = result.unwrap();
            // Height lock: 50 + 10 + 1 = 61
            assert_eq!(lock_points.height, 61);
            // Time lock: 600000 + (20 * 512) = 610240
            assert_eq!(lock_points.time, 610240);
        }

        #[test]
        fn test_max_sequence_value() {
            let client = Arc::new(MockBIP68Client::new());
            let mut coins_cache = CoinsViewCache::new(client.clone(), 1000);

            let prev_txid = Txid::all_zeros();
            let outpoint = OutPoint {
                txid: prev_txid,
                vout: 0,
            };
            client.add_utxo(outpoint, create_coin(50, 500000, false));

            // Maximum sequence value (0x0000ffff = 65535)
            const MAX_SEQ: u32 = 0x0000ffff;
            let tx = create_tx_with_sequence(MAX_SEQ);

            let current_height = 100000; // Very high to accommodate the lock
            let current_mtp = 10000000;

            let result =
                calculate_lock_points_at_tip(&tx, &mut coins_cache, current_height, current_mtp);

            assert!(result.is_ok());
            let lock_points = result.unwrap();
            // Lock height = 50 + 65535 + 1 = 65586
            assert_eq!(lock_points.height, 65586);
        }
    }
    */
}
