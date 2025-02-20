use bitcoin::{transaction::Version, Amount, Transaction, TxOut, Weight};
use std::collections::HashSet;

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

/// Transaction output types
#[derive(Debug, PartialEq, Eq)]
pub enum TxoutType {
    Nonstandard,
    // anyone can spend script.
    Anchor,
    PubKey,
    PubKeyHash,
    ScriptHash,
    Multisig,
    // unspendable OP_RETURN script that carries data.
    NullData,
    WitnessV0ScriptHash,
    WitnessV0KeyHash,
    WitnessV1Taproot,
    // Only for Witness versions not already defined above.
    WitnessUnknown,
}

#[derive(Debug)]
pub enum StandardTxError {
    Version,
    TxSize,
    TxSizeTooSmall,
    ScriptsigSize,
    Dust,
    MultiOpReturn,
    NonStandardWitness,
    BareMultisig,
    ScriptsigNotPushonly,
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
        return Err(StandardTxError::TxSize);
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
        let txout_type: TxoutType = todo!("IsStandard");

        match txout_type {
            TxoutType::NullData => {
                data_out += 1;

                // Only one OP_RETURN output is permitted
                if data_out > 1 {
                    return Err(StandardTxError::MultiOpReturn);
                }
            }
            TxoutType::Multisig if !permit_bare_multisig => {
                return Err(StandardTxError::BareMultisig);
            }
            _ => {}
        }

        if is_dust(&output, dust_relay_fee) {
            return Err(StandardTxError::Dust);
        }
    }

    Ok(())
}

/// Check if an output is dust (value too small compared to fee)
fn is_dust(output: &TxOut, dust_relay_fee: u64) -> bool {
    let dust_threshold = get_dust_threshold(output);
    output.value < dust_threshold
}

fn get_dust_threshold(txout: &TxOut) -> Amount {
    todo!()
}

/*
/// Check if a witness program is standard
fn is_standard_witness(
    witness: &Witness,
    version: u8,
    program: &[u8],
    rules: &StandardTxRules,
) -> bool {
    // Only version 0 and 1 are currently standard
    match version {
        0 => {
            match program.len() {
                20 => {
                    // P2WPKH
                    witness.stack.len() == 2
                }
                32 => {
                    // P2WSH
                    if witness.stack.len() > MAX_STANDARD_P2WSH_STACK_ITEMS {
                        return false;
                    }

                    let mut size = 0;
                    for item in &witness.stack {
                        if item.len() > MAX_STANDARD_P2WSH_STACK_ITEM_SIZE {
                            return false;
                        }
                        size += item.len();
                    }

                    // Check script size
                    if size > rules.max_script_size {
                        return false;
                    }

                    // Check sigops limit
                    if count_witness_sigops(&witness) > MAX_STANDARD_P2WSH_SIGOPS {
                        return false;
                    }

                    true
                }
                _ => false,
            }
        }
        1 => {
            // Taproot
            program.len() == 32 && witness.stack.len() <= 1
        }
        _ => false,
    }
}



/// Get the most restrictive script type from a set of outputs
fn get_most_restrictive_type(outputs: &[TxOutput]) -> TxoutType {
    let mut types = HashSet::new();
    for output in outputs {
        types.insert(get_script_type(&output.script_pubkey));
    }

    // Order from most to least restrictive
    let type_order = [
        TxoutType::WitnessV1Taproot,
        TxoutType::WitnessV0ScriptHash,
        TxoutType::WitnessV0KeyHash,
        TxoutType::ScriptHash,
        TxoutType::PubKeyHash,
        TxoutType::PubKey,
        TxoutType::Multisig,
        TxoutType::NullData,
    ];

    for tx_type in type_order {
        if types.contains(&tx_type) {
            return tx_type;
        }
    }

    TxoutType::Nonstandard
}
*/
