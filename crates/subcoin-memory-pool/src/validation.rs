//! Transaction validation for mempool acceptance.
//!
//! Implements Bitcoin Core's AcceptToMemoryPool (ATMP) flow:
//! 1. PreChecks: Basic validity, standardness, input availability
//! 2. PolicyScriptChecks: Script validation with STANDARD_SCRIPT_VERIFY_FLAGS
//! 3. ConsensusScriptChecks: Script validation with consensus flags
//! 4. Finalize: Add to mempool, update indices

use crate::arena::TxMemPoolEntry;
use crate::coins_view::CoinsViewCache;
use crate::inner::MemPoolInner;
use crate::options::MemPoolOptions;
use crate::policy::is_standard_tx;
use crate::types::{ConflictSet, EntryId, FeeRate, LockPoints, MempoolError};
use bitcoin::absolute::{LOCK_TIME_THRESHOLD, LockTime};
use bitcoin::{Transaction, Weight};
use sc_client_api::HeaderBackend;
use sp_api::ProvideRuntimeApi;
use sp_runtime::traits::Block as BlockT;
use std::collections::{HashMap, HashSet};
use std::convert::TryFrom;
use std::sync::Arc;
use subcoin_primitives::SubcoinRuntimeApi;
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
    pub ancestors: HashSet<EntryId>,

    /// Conflicting mempool transactions (txids).
    pub conflicts: HashSet<bitcoin::Txid>,

    /// Full conflict set for RBF (if any).
    pub conflict_set: Option<ConflictSet>,
}

impl ValidationWorkspace {
    /// Create new workspace for transaction validation.
    pub fn new(tx: Transaction) -> Self {
        let tx_weight = tx.weight();
        let vsize = tx_weight.to_vbytes_ceil() as i64;

        Self {
            tx: Arc::new(tx),
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
    _best_block: Block::Hash,
) -> Result<(), MempoolError>
where
    Block: BlockT,
    Client: ProvideRuntimeApi<Block> + HeaderBackend<Block> + Send + Sync,
    Client::Api: SubcoinRuntimeApi<Block>,
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

    // 6. Check for conflicts with mempool transactions
    for input in &tx.input {
        if let Some(conflicting_txid) = inner.get_conflict_tx(&input.previous_output) {
            // For now, reject conflicts (TODO: RBF support)
            return Err(MempoolError::TxConflict(format!(
                "spends output already spent by {conflicting_txid}"
            )));
        }
    }

    // 7. Batch-prefetch all input coins
    let outpoints: Vec<_> = tx.input.iter().map(|txin| txin.previous_output).collect();
    coins_cache.ensure_coins(&outpoints)?;

    let mut prev_outputs: HashMap<_, _> = HashMap::with_capacity(outpoints.len());

    // 8. Check all inputs are available
    for outpoint in &outpoints {
        if !coins_cache.have_coin(outpoint) {
            return Err(MempoolError::MissingInputs);
        }
    }

    // 9. Calculate fees and check for negative fee
    let mut input_value = bitcoin::Amount::ZERO;
    let mut spends_coinbase = false;

    for outpoint in &outpoints {
        let coin = coins_cache
            .get_coin(outpoint)?
            .ok_or(MempoolError::MissingInputs)?;

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
        return Err(MempoolError::FeeTooLow(format!(
            "fee {base_fee} < min {min_fee}"
        )));
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

    // 13. Store computed values in workspace
    ws.base_fee = base_fee;
    ws.modified_fee = base_fee; // TODO: Apply priority deltas
    ws.spends_coinbase = spends_coinbase;
    ws.sigop_cost = sigop_cost;
    ws.lock_points = LockPoints {
        height: current_height as i32,
        time: i64::from(current_time_secs),
        max_input_block: None,
    };

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
pub fn finalize_tx<Block, Client>(
    ws: ValidationWorkspace,
    inner: &mut MemPoolInner,
    coins_cache: &mut CoinsViewCache<Block, Client>,
    current_height: u32,
    current_time: i64,
    sequence: u64,
) -> Result<EntryId, MempoolError>
where
    Block: BlockT,
    Client: ProvideRuntimeApi<Block> + HeaderBackend<Block> + Send + Sync,
    Client::Api: SubcoinRuntimeApi<Block>,
{
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
    Client::Api: SubcoinRuntimeApi<Block>,
{
    let tx = &ws.tx;

    for (input_index, txin) in tx.input.iter().enumerate() {
        let coin = coins_cache
            .get_coin(&txin.previous_output)?
            .ok_or(MempoolError::MissingInputs)?;

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
    coins_cache: &CoinsViewCache<Block, Client>,
    options: &MemPoolOptions,
) -> Result<ConflictSet, MempoolError>
where
    Block: BlockT,
    Client: ProvideRuntimeApi<Block> + HeaderBackend<Block> + Send + Sync,
    Client::Api: SubcoinRuntimeApi<Block>,
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
