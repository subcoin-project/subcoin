use bitcoin::Transaction;
use bitcoin::absolute::{LOCK_TIME_THRESHOLD, LockTime};

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
