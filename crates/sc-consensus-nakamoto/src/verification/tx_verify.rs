use super::MAX_BLOCK_WEIGHT;
use bitcoin::absolute::{LOCK_TIME_THRESHOLD, LockTime};
use bitcoin::blockdata::weight::WITNESS_SCALE_FACTOR;
use bitcoin::{Amount, Transaction, Weight};
use std::collections::HashSet;

// MinCoinbaseScriptLen is the minimum length a coinbase script can be.
const MIN_COINBASE_SCRIPT_LEN: usize = 2;

// MaxCoinbaseScriptLen is the maximum length a coinbase script can be.
const MAX_COINBASE_SCRIPT_LEN: usize = 100;

/// Transaction verification error.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Transaction has no inputs")]
    EmptyInput,
    #[error("Transaction has no outputs")]
    EmptyOutput,
    #[error("Transaction is too large")]
    TransactionOversize,
    #[error("Transaction contains duplicate inputs at index {0}")]
    DuplicateTxInput(usize),
    #[error("Output value (0) is too large")]
    OutputValueTooLarge(Amount),
    #[error("Total output value (0) is too large")]
    TotalOutputValueTooLarge(Amount),
    #[error(
        "Coinbase transaction script length of {0} is out of range \
        (min: {MIN_COINBASE_SCRIPT_LEN}, max: {MAX_COINBASE_SCRIPT_LEN})"
    )]
    BadCoinbaseLength(usize),
    #[error("Transaction input refers to a previous output that is null")]
    PreviousOutputNull,
}

/// Checks whether the transaction is final at the given height and block time.
pub fn is_final_tx(tx: &Transaction, height: u32, block_time: u32) -> bool {
    if tx.lock_time == LockTime::ZERO {
        return true;
    }

    let lock_time = if tx.lock_time.to_consensus_u32() < LOCK_TIME_THRESHOLD {
        height
    } else {
        block_time
    };

    if tx.lock_time.to_consensus_u32() < lock_time {
        return true;
    }

    // Even if tx.nLockTime isn't satisfied by nBlockHeight/nBlockTime, a
    // transaction is still considered final if all inputs' nSequence ==
    // SEQUENCE_FINAL (0xffffffff), in which case nLockTime is ignored.
    //
    // Because of this behavior OP_CHECKLOCKTIMEVERIFY/CheckLockTime() will
    // also check that the spending input's nSequence != SEQUENCE_FINAL,
    // ensuring that an unsatisfied nLockTime value will actually cause
    // IsFinalTx() to return false here:
    tx.input.iter().all(|txin| txin.sequence.is_final())
}

/// Counts the sigops for this transaction using legacy counting.
pub fn get_legacy_sig_op_count(tx: &Transaction) -> usize {
    tx.input
        .iter()
        .map(|txin| txin.script_sig.count_sigops_legacy())
        .sum::<usize>()
        + tx.output
            .iter()
            .map(|txout| txout.script_pubkey.count_sigops_legacy())
            .sum::<usize>()
}
