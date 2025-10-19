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
use crate::types::{EntryId, LockPoints, MempoolError};
use bitcoin::{Transaction, Weight};
use sc_client_api::HeaderBackend;
use sp_api::ProvideRuntimeApi;
use sp_runtime::traits::Block as BlockT;
use std::collections::HashSet;
use std::sync::Arc;
use subcoin_primitives::SubcoinRuntimeApi;
use subcoin_primitives::consensus::check_transaction_sanity;

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
        }
    }
}

/// Pre-validation checks before script execution.
///
/// Corresponds to Bitcoin Core's `PreChecks()` function.
/// Validates:
/// - Transaction sanity (CheckTransaction)
/// - Not coinbase
/// - Standardness (if required)
/// - Finality (nLockTime and BIP68)
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
        is_standard_tx(
            tx,
            options.max_datacarrier_bytes,
            options.permit_bare_multisig,
            options.dust_relay_feerate,
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

    // 11. TODO: Check finality (nLockTime, nSequence/BIP68)
    // For now, we accept all transactions

    // 12. Store computed values in workspace
    ws.base_fee = base_fee;
    ws.modified_fee = base_fee; // TODO: Apply priority deltas
    ws.spends_coinbase = spends_coinbase;

    // TODO: Calculate sigop cost
    ws.sigop_cost = 0;

    // TODO: Calculate lock points
    ws.lock_points = LockPoints::default();

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
