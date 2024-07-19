use super::MAX_BLOCK_WEIGHT;
use bitcoin::absolute::{LockTime, LOCK_TIME_THRESHOLD};
use bitcoin::{Amount, Transaction};
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
    BadTransactionLength,
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
    BadCoinbaseScriptSigLength(usize),
    #[error("Transaction input refers to previous output that is null")]
    BadTxInput,
}

pub fn is_final(tx: &Transaction, height: u32, block_time: u32) -> bool {
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

    tx.input.iter().all(|txin| txin.sequence.is_final())
}

pub fn check_transaction_sanity(tx: &Transaction) -> Result<(), Error> {
    if tx.input.is_empty() {
        return Err(Error::EmptyInput);
    }

    if tx.output.is_empty() {
        return Err(Error::EmptyOutput);
    }

    if tx.weight() > MAX_BLOCK_WEIGHT {
        return Err(Error::BadTransactionLength);
    }

    // Check for duplicate inputs.
    let mut seen_inputs = HashSet::new();
    for (index, txin) in tx.input.iter().enumerate() {
        if !seen_inputs.insert(txin.previous_output) {
            return Err(Error::DuplicateTxInput(index));
        }
    }

    let mut total_output_value = Amount::ZERO;
    tx.output.iter().try_for_each(|txout| {
        if txout.value > Amount::MAX_MONEY {
            return Err(Error::OutputValueTooLarge(txout.value));
        }

        total_output_value += txout.value;

        if total_output_value > Amount::MAX_MONEY {
            return Err(Error::TotalOutputValueTooLarge(total_output_value));
        }

        Ok(())
    })?;

    // Coinbase script length must be between min and max length.
    if tx.is_coinbase() {
        let script_sig_len = tx.input[0].script_sig.len();

        if !(MIN_COINBASE_SCRIPT_LEN..=MAX_COINBASE_SCRIPT_LEN).contains(&script_sig_len) {
            return Err(Error::BadCoinbaseScriptSigLength(script_sig_len));
        }
    } else {
        // Previous transaction outputs referenced by the inputs to this
        // transaction must not be null.
        if tx.input.iter().any(|txin| txin.previous_output.is_null()) {
            return Err(Error::BadTxInput);
        }
    }

    Ok(())
}
