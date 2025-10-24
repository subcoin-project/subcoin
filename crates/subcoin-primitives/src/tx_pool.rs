//! Bitcoin transaction pool abstraction for network integration.

use bitcoin::{Amount, Transaction, Txid};
use std::sync::Arc;

/// Result of transaction validation.
#[derive(Debug, Clone)]
pub enum TxValidationResult {
    /// Transaction accepted into mempool.
    Accepted {
        txid: Txid,
        /// Fee rate in sat/kvB for relay decisions.
        fee_rate: u64,
    },
    /// Transaction rejected.
    Rejected {
        txid: Txid,
        reason: RejectionReason,
    },
}

/// Classification of rejection reasons for peer penalty policy.
#[derive(Debug, Clone)]
pub enum RejectionReason {
    /// Soft rejection - don't penalize peer.
    Soft(SoftRejection),
    /// Hard rejection - penalize peer for protocol violation.
    Hard(HardRejection),
}

impl RejectionReason {
    /// Returns true if the peer should be penalized for this rejection.
    pub fn should_penalize_peer(&self) -> bool {
        matches!(self, Self::Hard(_))
    }
}

/// Soft rejections - legitimate reasons that don't indicate misbehavior.
#[derive(Debug, Clone)]
pub enum SoftRejection {
    /// Transaction already in mempool.
    AlreadyInMempool,
    /// Missing parent transactions (might arrive later).
    MissingInputs {
        parents: Vec<Txid>,
    },
    /// Fee rate too low for relay.
    FeeTooLow {
        min_kvb: u64,
        actual_kvb: u64,
    },
    /// Mempool is full.
    MempoolFull,
    /// Too many ancestor/descendant transactions.
    TooManyAncestors(usize),
    TooManyDescendants(usize),
    /// Transaction conflicts with mempool.
    TxConflict(String),
    /// RBF-related issues.
    NoConflictToReplace,
    TxNotReplaceable,
    TooManyReplacements(usize),
    NewUnconfirmedInput,
    InsufficientFee(String),
    /// Package relay issues.
    PackageTooLarge(usize, usize),
    PackageSizeTooLarge(u64),
    PackageCyclicDependencies,
    PackageFeeTooLow(String),
    PackageTxValidationFailed(Txid, String),
    PackageRelayDisabled,
}

/// Hard rejections - indicate protocol violations or malformed transactions.
#[derive(Debug, Clone)]
pub enum HardRejection {
    /// Coinbase transaction not allowed in mempool.
    Coinbase,
    /// Transaction is non-standard.
    NotStandard(String),
    TxVersionNotStandard,
    TxSizeTooSmall,
    /// Transaction is non-final.
    NonFinal,
    NonBIP68Final,
    /// Too many signature operations.
    TooManySigops(i64),
    /// Fee calculation errors.
    NegativeFee,
    FeeOverflow,
    InvalidFeeRate(String),
    /// Ancestor/descendant size limits.
    AncestorSizeTooLarge(i64),
    DescendantSizeTooLarge(i64),
    /// Script validation failed.
    ScriptValidationFailed(String),
    /// Other validation errors.
    TxError(String),
    RuntimeApi(String),
}

/// Mempool statistics.
#[derive(Debug, Clone)]
pub struct TxPoolInfo {
    /// Number of transactions in mempool.
    pub size: usize,
    /// Total virtual size of all transactions.
    pub bytes: u64,
    /// Total fees of all transactions.
    pub usage: u64,
    /// Current minimum relay fee rate in sat/kvB.
    pub min_fee_rate: u64,
}

/// Bitcoin transaction pool trait for network integration.
///
/// This trait abstracts mempool operations needed by the network layer,
/// avoiding circular dependencies and enabling testing with mock implementations.
///
/// All methods are synchronous - the caller can decide whether to run them
/// on a blocking executor (e.g., Substrate's `spawn_blocking`) or inline.
pub trait TxPool: Send + Sync + 'static {
    /// Validate and potentially accept a transaction into the mempool.
    ///
    /// This is a blocking operation that holds internal locks and performs
    /// script validation. The caller should run this on a blocking executor
    /// if needed (e.g., `task_manager.spawn_blocking()`).
    fn validate_transaction(&self, tx: Transaction) -> TxValidationResult;

    /// Check if transaction is already in mempool.
    fn contains(&self, txid: &Txid) -> bool;

    /// Get transaction from mempool if present.
    fn get(&self, txid: &Txid) -> Option<Arc<Transaction>>;

    /// Get transactions that haven't been broadcast yet.
    /// Returns (txid, fee_rate) pairs.
    fn get_unbroadcast(&self) -> Vec<(Txid, u64)>;

    /// Mark transactions as broadcast to peers.
    fn mark_broadcast(&self, txids: &[Txid]);

    /// Iterate over all transaction IDs with their fee rates.
    /// Returns (txid, fee_rate) pairs sorted by mining priority.
    fn iter_txids(&self) -> Box<dyn Iterator<Item = (Txid, u64)> + Send>;

    /// Get mempool statistics.
    fn info(&self) -> TxPoolInfo;
}

/// No-op transaction pool for backward compatibility.
///
/// This default implementation allows existing code to compile without
/// requiring immediate mempool integration.
#[derive(Debug, Default, Clone)]
pub struct NoOpTxPool;

impl TxPool for NoOpTxPool {
    fn validate_transaction(&self, tx: Transaction) -> TxValidationResult {
        TxValidationResult::Rejected {
            txid: tx.compute_txid(),
            reason: RejectionReason::Soft(SoftRejection::PackageRelayDisabled),
        }
    }

    fn contains(&self, _txid: &Txid) -> bool {
        false
    }

    fn get(&self, _txid: &Txid) -> Option<Arc<Transaction>> {
        None
    }

    fn get_unbroadcast(&self) -> Vec<(Txid, u64)> {
        Vec::new()
    }

    fn mark_broadcast(&self, _txids: &[Txid]) {}

    fn iter_txids(&self) -> Box<dyn Iterator<Item = (Txid, u64)> + Send> {
        Box::new(std::iter::empty())
    }

    fn info(&self) -> TxPoolInfo {
        TxPoolInfo {
            size: 0,
            bytes: 0,
            usage: 0,
            min_fee_rate: 0,
        }
    }
}

/// Helper: Calculate fee rate from amount and vsize with overflow protection.
///
/// Returns fee rate in sat/kvB.
pub fn fee_rate_from_amount_vsize(fee: Amount, vsize: i64) -> Result<u64, &'static str> {
    if vsize <= 0 {
        return Err("vsize must be positive");
    }

    let fee_sat = fee.to_sat();
    let vsize_u64 = vsize as u64;

    // Calculate fee_sat * 1000 / vsize with overflow protection
    let numerator = fee_sat
        .checked_mul(1000)
        .ok_or("Fee rate calculation overflow")?;

    Ok(numerator / vsize_u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fee_rate_calculation() {
        // 1000 sat fee, 250 vbytes = 4000 sat/kvB
        let fee = Amount::from_sat(1000);
        assert_eq!(fee_rate_from_amount_vsize(fee, 250).unwrap(), 4000);

        // 500 sat fee, 200 vbytes = 2500 sat/kvB
        let fee = Amount::from_sat(500);
        assert_eq!(fee_rate_from_amount_vsize(fee, 200).unwrap(), 2500);

        // Zero vsize should error
        let fee = Amount::from_sat(1000);
        assert!(fee_rate_from_amount_vsize(fee, 0).is_err());

        // Negative vsize should error
        assert!(fee_rate_from_amount_vsize(fee, -1).is_err());
    }

    #[test]
    fn test_rejection_reason_penalty() {
        let soft = RejectionReason::Soft(SoftRejection::AlreadyInMempool);
        assert!(!soft.should_penalize_peer());

        let hard = RejectionReason::Hard(HardRejection::Coinbase);
        assert!(hard.should_penalize_peer());
    }

    #[test]
    fn test_noop_pool() {
        let pool = NoOpTxPool;
        let tx = Transaction {
            version: bitcoin::transaction::Version::TWO,
            lock_time: bitcoin::absolute::LockTime::ZERO,
            input: vec![],
            output: vec![],
        };

        let result = pool.validate_transaction(tx);
        assert!(matches!(result, TxValidationResult::Rejected { .. }));
        assert!(!pool.contains(&Txid::all_zeros()));
        assert!(pool.get(&Txid::all_zeros()).is_none());
        assert_eq!(pool.get_unbroadcast().len(), 0);
        assert_eq!(pool.info().size, 0);
    }
}
