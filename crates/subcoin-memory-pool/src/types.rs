//! Core type definitions for the mempool.

use bitcoin::{Amount, BlockHash, Transaction, Txid};
use slotmap::DefaultKey;
use std::collections::HashSet;
use std::sync::Arc;

/// Handle to entry in mempool arena (not an iterator).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EntryId(pub(crate) DefaultKey);

/// Fee rate in satoshis per virtual kilobyte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct FeeRate(pub u64);

impl FeeRate {
    /// Create fee rate from satoshis per virtual byte.
    pub fn from_sat_per_vb(sat_vb: u64) -> Self {
        Self(sat_vb.checked_mul(1000).expect("Fee rate overflow"))
    }

    /// Create fee rate from satoshis per kilovirtual byte.
    pub fn from_sat_per_kvb(sat_kvb: u64) -> Self {
        Self(sat_kvb)
    }

    /// Get fee for given virtual size.
    pub fn get_fee(&self, vsize: i64) -> Amount {
        let fee_sat = (self
            .0
            .checked_mul(vsize as u64)
            .expect("Fee calculation overflow"))
        .checked_div(1000)
        .unwrap_or(0);
        Amount::from_sat(fee_sat)
    }

    /// Get the fee rate in satoshis per kilovirtual byte.
    pub fn as_sat_per_kvb(&self) -> u64 {
        self.0
    }
}

/// Mempool-specific errors.
#[derive(Debug, thiserror::Error)]
pub enum MempoolError {
    #[error("Transaction already in mempool")]
    AlreadyInMempool,

    #[error("Coinbase transaction not allowed")]
    Coinbase,

    #[error("Missing inputs")]
    MissingInputs,

    #[error("Fee too low: {0}")]
    FeeTooLow(String),

    #[error("Too many sigops: {0}")]
    TooManySigops(i64),

    #[error("Too many ancestors: {0}")]
    TooManyAncestors(usize),

    #[error("Ancestor size too large: {0}")]
    AncestorSizeTooLarge(i64),

    #[error("Too many descendants: {0}")]
    TooManyDescendants(usize),

    #[error("Descendant size too large: {0}")]
    DescendantSizeTooLarge(i64),

    #[error("Not standard: {0}")]
    NotStandard(String),

    #[error("Transaction version not standard")]
    TxVersionNotStandard,

    #[error("Transaction size too small")]
    TxSizeTooSmall,

    #[error("Non-final transaction")]
    NonFinal,

    #[error("Non-BIP68-final")]
    NonBIP68Final,

    #[error("Negative fee")]
    NegativeFee,

    #[error("Overflow in fee calculation")]
    FeeOverflow,

    #[error("Mempool is full")]
    MempoolFull,

    #[error("Transaction conflicts with mempool: {0}")]
    TxConflict(String),

    #[error("Script validation failed: {0}")]
    ScriptValidationFailed(String),

    #[error("No conflicting transaction to replace")]
    NoConflictToReplace,

    #[error("Conflicting transaction is not replaceable (doesn't signal BIP125)")]
    TxNotReplaceable,

    #[error("Too many transactions to replace: {0} (max 100)")]
    TooManyReplacements(usize),

    #[error("Replacement introduces new unconfirmed inputs")]
    NewUnconfirmedInput,

    #[error("Missing conflict transaction in mempool")]
    MissingConflict,

    #[error("Insufficient fee: {0}")]
    InsufficientFee(String),

    #[error("Package too large: {0} transactions (max {1})")]
    PackageTooLarge(usize, usize),

    #[error("Package exceeds size limit: {0} vbytes")]
    PackageSizeTooLarge(u64),

    #[error("Package has cyclic dependencies")]
    PackageCyclicDependencies,

    #[error("Package feerate too low: {0}")]
    PackageFeeTooLow(String),

    #[error("Package validation failed for tx {0}: {1}")]
    PackageTxValidationFailed(bitcoin::Txid, String),

    #[error("Package relay is disabled")]
    PackageRelayDisabled,

    #[error(transparent)]
    TxError(#[from] subcoin_primitives::consensus::TxError),

    #[error("Runtime API error: {0}")]
    RuntimeApi(String),
}

/// Lock points for BIP68/BIP112 validation.
#[derive(Debug, Clone, Default)]
pub struct LockPoints {
    /// Height at which transaction becomes valid.
    pub height: i32,
    /// Time at which transaction becomes valid.
    pub time: i64,
    /// Highest block containing an input of this transaction.
    pub max_input_block: Option<BlockHash>,
}

/// Result of transaction pre-validation.
pub struct ValidationResult {
    /// Base fee paid by transaction.
    pub base_fee: Amount,
    /// Signature operation cost.
    pub sigop_cost: i64,
    /// Lock points for BIP68/112.
    pub lock_points: LockPoints,
    /// Set of ancestor entry IDs in mempool.
    pub ancestors: HashSet<EntryId>,
    /// Set of conflicting transaction IDs.
    pub conflicts: HashSet<Txid>,
    /// Whether this transaction spends a coinbase output.
    pub spends_coinbase: bool,
}

/// Reason for removing transactions from mempool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovalReason {
    /// Included in a block.
    Block,
    /// Chain reorganization.
    Reorg,
    /// Conflicted with another transaction.
    Conflict,
    /// Replaced by higher-fee transaction (RBF).
    Replaced,
    /// Evicted due to mempool size limit.
    SizeLimit,
    /// Expired (too old).
    Expiry,
}

impl RemovalReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::Reorg => "reorg",
            Self::Conflict => "conflict",
            Self::Replaced => "replaced",
            Self::SizeLimit => "sizelimit",
            Self::Expiry => "expiry",
        }
    }
}

/// Set of transactions being replaced by RBF.
#[derive(Debug, Clone)]
pub struct ConflictSet {
    /// Direct conflicts (txs spending same outputs).
    pub direct_conflicts: HashSet<EntryId>,

    /// All affected (conflicts + descendants).
    pub all_conflicts: HashSet<EntryId>,

    /// Transactions to remove (for coins cache cleanup).
    /// Must capture BEFORE calling remove_staged().
    pub removed_transactions: Vec<Arc<Transaction>>,

    /// Total fees of all replaced transactions.
    pub replaced_fees: bitcoin::Amount,

    /// Total size of all replaced transactions.
    pub replaced_size: i64,
}

/// A package of related transactions to be validated together.
#[derive(Debug, Clone)]
pub struct Package {
    pub transactions: Vec<Arc<Transaction>>,
}

/// Package validation result.
#[derive(Debug)]
pub struct PackageValidationResult {
    pub accepted: Vec<Txid>,
    pub package_feerate: FeeRate,
}
