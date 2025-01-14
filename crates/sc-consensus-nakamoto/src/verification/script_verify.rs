use super::Error;
use crate::chain_params::ChainParams;
use bitcoin::{BlockHash, TxOut};
use std::ffi::c_uint;

/// Returns the script validation flags for the specified block.
///
/// <https://github.com/bitcoin/bitcoin/blob/6f9db1ebcab4064065ccd787161bf2b87e03cc1f/src/validation.cpp#L2360>
pub fn get_block_script_flags(
    height: u32,
    block_hash: BlockHash,
    chain_params: &ChainParams,
) -> c_uint {
    if let Some(flag) = chain_params
        .script_flag_exceptions
        .get(&block_hash)
        .copied()
    {
        return flag;
    }

    let mut flags = bitcoinconsensus::VERIFY_P2SH | bitcoinconsensus::VERIFY_WITNESS;

    // Enforce the DERSIG (BIP66) rule
    if height >= chain_params.params.bip66_height {
        flags |= bitcoinconsensus::VERIFY_DERSIG;
    }

    // Enforce CHECKLOCKTIMEVERIFY (BIP65)
    if height >= chain_params.params.bip65_height {
        flags |= bitcoinconsensus::VERIFY_CHECKLOCKTIMEVERIFY;
    }

    // Enforce CHECKSEQUENCEVERIFY (BIP112)
    if height >= chain_params.csv_height {
        flags |= bitcoinconsensus::VERIFY_CHECKSEQUENCEVERIFY;
    }

    // Enforce BIP147 NULLDUMMY (activated simultaneously with segwit)
    if height >= chain_params.segwit_height {
        flags |= bitcoinconsensus::VERIFY_NULLDUMMY;
    }

    flags
}

pub fn verify_input_script(
    spent_output: &TxOut,
    spending_transaction: &[u8],
    input_index: usize,
    flags: u32,
) -> Result<(), Error> {
    let script_verify_result = bitcoinconsensus::verify_with_flags(
        spent_output.script_pubkey.as_bytes(),
        spent_output.value.to_sat(),
        spending_transaction,
        input_index,
        flags,
    );

    match script_verify_result {
        Ok(()) | Err(bitcoinconsensus::Error::ERR_SCRIPT) => Ok(()),
        Err(script_error) => Err(script_error.into()),
    }
}
