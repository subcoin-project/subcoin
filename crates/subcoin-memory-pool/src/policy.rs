use bitcoin::transaction::Version;
use bitcoin::{Amount, Script, Transaction, TxOut, Weight};
use subcoin_script::solver::{TxoutType, solve};

const TX_MAX_STANDARD_VERSION: Version = Version(3);

/// The maximum weight for transactions we're willing to relay/mine.
const MAX_STANDARD_TX_WEIGHT: Weight = Weight::from_wu(400000);

/// The maximum size of s standard ScripgSig.
const MAX_STANDARD_SCRIPTSIG_SIZE: usize = 1650;

/// The minimum non-witness size for transactions we're willing to relay/mine: one larger than 64.
pub const MIN_STANDARD_TX_NONWITNESS_SIZE: usize = 65;

/// Maximum number of witness stack items in a standard P2WSH transaction
const MAX_STANDARD_P2WSH_STACK_ITEMS: usize = 100;

/// Maximum size of a standard witness stack item
const MAX_STANDARD_P2WSH_STACK_ITEM_SIZE: usize = 80;

/// Maximum number of sigops in a standard P2WSH transaction
const MAX_STANDARD_P2WSH_SIGOPS: usize = 15;

#[derive(Debug)]
pub enum StandardTxError {
    Version,
    TxSizeTooLarge,
    TxSizeTooSmall,
    ScriptsigSize,
    Dust,
    MultiOpReturn,
    NonStandardWitness,
    BareMultisig,
    ScriptsigNotPushonly,
    ScriptPubkey,
}

/// Check whether a transaction is standard
pub fn is_standard_tx(
    tx: &Transaction,
    max_datacarrier_bytes: usize,
    permit_bare_multisig: bool,
    dust_relay_fee: u64,
) -> Result<(), StandardTxError> {
    if tx.version < Version::ONE || tx.version > TX_MAX_STANDARD_VERSION {
        return Err(StandardTxError::Version);
    }

    if tx.weight() > MAX_STANDARD_TX_WEIGHT {
        return Err(StandardTxError::TxSizeTooLarge);
    }

    if tx.base_size() < MIN_STANDARD_TX_NONWITNESS_SIZE {
        return Err(StandardTxError::TxSizeTooSmall);
    }

    // Check for standard input scripts
    for input in &tx.input {
        if input.script_sig.len() > MAX_STANDARD_SCRIPTSIG_SIZE {
            return Err(StandardTxError::ScriptsigSize);
        }

        if !input.script_sig.is_push_only() {
            return Err(StandardTxError::ScriptsigNotPushonly);
        }
    }

    // Track number of OP_RETURN outputs
    let mut data_out = 0;

    for output in &tx.output {
        let which_type = solve(&output.script_pubkey);

        if !is_standard(&which_type, &output.script_pubkey, max_datacarrier_bytes) {
            return Err(StandardTxError::ScriptPubkey);
        }

        match &which_type {
            TxoutType::NullData => {
                data_out += 1;

                // Only one OP_RETURN output is permitted
                if data_out > 1 {
                    return Err(StandardTxError::MultiOpReturn);
                }
            }
            TxoutType::Multisig { .. } if !permit_bare_multisig => {
                return Err(StandardTxError::BareMultisig);
            }
            _ => {}
        }

        if is_dust(output, dust_relay_fee) {
            return Err(StandardTxError::Dust);
        }
    }

    Ok(())
}

fn is_standard(
    which_type: &TxoutType,
    script_pubkey: &Script,
    max_datacarrier_bytes: usize,
) -> bool {
    match which_type {
        TxoutType::NonStandard => return false,
        TxoutType::Multisig {
            required_sigs: m,
            keys_count: n,
            keys: _,
        } => {
            if *n < 1 || *n > 3 {
                return false;
            }

            if *m < 1 || *m > *n {
                return false;
            }
        }
        TxoutType::NullData => {
            if max_datacarrier_bytes > 0 && script_pubkey.len() > max_datacarrier_bytes {
                return false;
            }
        }
        _ => {}
    }

    true
}

/// Check if an output is dust (value too small compared to fee)
fn is_dust(output: &TxOut, dust_relay_fee: u64) -> bool {
    let dust_threshold = get_dust_threshold(output);
    output.value < dust_threshold
}

fn get_dust_threshold(txout: &TxOut) -> Amount {
    todo!()
}
