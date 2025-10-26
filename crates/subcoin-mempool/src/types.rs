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

    /// Calculate fee rate from amount and vsize with overflow protection.
    ///
    /// Returns fee rate in sat/kvB.
    pub fn from_amount_and_vsize(fee: Amount, vsize: i64) -> Result<Self, &'static str> {
        if vsize <= 0 {
            return Err("vsize must be positive");
        }

        let fee_sat = fee.to_sat();
        let vsize_u64 = vsize as u64;

        // Calculate fee_sat * 1000 / vsize with overflow protection
        let numerator = fee_sat
            .checked_mul(1000)
            .ok_or("Fee rate calculation overflow")?;

        Ok(Self(numerator / vsize_u64))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fee_rate_from_amount_and_vsize() {
        // 1000 sat fee, 250 vbytes = 4000 sat/kvB
        let fee = Amount::from_sat(1000);
        assert_eq!(
            FeeRate::from_amount_and_vsize(fee, 250)
                .unwrap()
                .as_sat_per_kvb(),
            4000
        );

        // 500 sat fee, 200 vbytes = 2500 sat/kvB
        let fee = Amount::from_sat(500);
        assert_eq!(
            FeeRate::from_amount_and_vsize(fee, 200)
                .unwrap()
                .as_sat_per_kvb(),
            2500
        );

        // Zero vsize should error
        let fee = Amount::from_sat(1000);
        assert!(FeeRate::from_amount_and_vsize(fee, 0).is_err());

        // Negative vsize should error
        assert!(FeeRate::from_amount_and_vsize(fee, -1).is_err());
    }
}
