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

// <https://github.com/bitcoin/bitcoin/blob/6f9db1ebcab4064065ccd787161bf2b87e03cc1f/src/consensus/tx_check.cpp#L11>
pub fn check_transaction_sanity(tx: &Transaction) -> Result<(), Error> {
    if tx.input.is_empty() {
        return Err(Error::EmptyInput);
    }

    if tx.output.is_empty() {
        return Err(Error::EmptyOutput);
    }

    if Weight::from_wu((tx.base_size() * WITNESS_SCALE_FACTOR) as u64) > MAX_BLOCK_WEIGHT {
        return Err(Error::TransactionOversize);
    }

    let mut value_out = Amount::ZERO;
    tx.output.iter().try_for_each(|txout| {
        if txout.value > Amount::MAX_MONEY {
            return Err(Error::OutputValueTooLarge(txout.value));
        }

        value_out += txout.value;

        if value_out > Amount::MAX_MONEY {
            return Err(Error::TotalOutputValueTooLarge(value_out));
        }

        Ok(())
    })?;

    // Check for duplicate inputs.
    let mut seen_inputs = HashSet::with_capacity(tx.input.len());
    for (index, txin) in tx.input.iter().enumerate() {
        if !seen_inputs.insert(txin.previous_output) {
            return Err(Error::DuplicateTxInput(index));
        }
    }

    // Coinbase script length must be between min and max length.
    if tx.is_coinbase() {
        let script_sig_len = tx.input[0].script_sig.len();

        if !(MIN_COINBASE_SCRIPT_LEN..=MAX_COINBASE_SCRIPT_LEN).contains(&script_sig_len) {
            return Err(Error::BadCoinbaseLength(script_sig_len));
        }
    } else {
        // Previous transaction outputs referenced by the inputs to this
        // transaction must not be null.
        if tx.input.iter().any(|txin| txin.previous_output.is_null()) {
            return Err(Error::PreviousOutputNull);
        }
    }

    Ok(())
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
