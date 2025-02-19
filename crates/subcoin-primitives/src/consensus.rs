use crate::MAX_BLOCK_WEIGHT;
use bitcoin::blockdata::weight::WITNESS_SCALE_FACTOR;
use bitcoin::{Amount, Transaction, Weight};
use std::collections::HashSet;

// MinCoinbaseScriptLen is the minimum length a coinbase script can be.
const MIN_COINBASE_SCRIPT_LEN: usize = 2;

// MaxCoinbaseScriptLen is the maximum length a coinbase script can be.
const MAX_COINBASE_SCRIPT_LEN: usize = 100;

/// Transaction validation error.
#[derive(Debug, thiserror::Error)]
pub enum TxError {
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

/// Basic checks that don't depend on any context.
// <https://github.com/bitcoin/bitcoin/blob/6f9db1ebcab4064065ccd787161bf2b87e03cc1f/src/consensus/tx_check.cpp#L11>
pub fn check_transaction_sanity(tx: &Transaction) -> Result<(), TxError> {
    if tx.input.is_empty() {
        return Err(TxError::EmptyInput);
    }

    if tx.output.is_empty() {
        return Err(TxError::EmptyOutput);
    }

    if Weight::from_wu((tx.base_size() * WITNESS_SCALE_FACTOR) as u64) > MAX_BLOCK_WEIGHT {
        return Err(TxError::TransactionOversize);
    }

    let mut value_out = Amount::ZERO;
    tx.output.iter().try_for_each(|txout| {
        if txout.value > Amount::MAX_MONEY {
            return Err(TxError::OutputValueTooLarge(txout.value));
        }

        value_out += txout.value;

        if value_out > Amount::MAX_MONEY {
            return Err(TxError::TotalOutputValueTooLarge(value_out));
        }

        Ok(())
    })?;

    // Check for duplicate inputs.
    let mut seen_inputs = HashSet::with_capacity(tx.input.len());
    for (index, txin) in tx.input.iter().enumerate() {
        if !seen_inputs.insert(txin.previous_output) {
            return Err(TxError::DuplicateTxInput(index));
        }
    }

    // Coinbase script length must be between min and max length.
    if tx.is_coinbase() {
        let script_sig_len = tx.input[0].script_sig.len();

        if !(MIN_COINBASE_SCRIPT_LEN..=MAX_COINBASE_SCRIPT_LEN).contains(&script_sig_len) {
            return Err(TxError::BadCoinbaseLength(script_sig_len));
        }
    } else {
        // Previous transaction outputs referenced by the inputs to this
        // transaction must not be null.
        if tx.input.iter().any(|txin| txin.previous_output.is_null()) {
            return Err(TxError::PreviousOutputNull);
        }
    }

    Ok(())
}
