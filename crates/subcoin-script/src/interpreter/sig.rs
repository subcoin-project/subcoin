use super::SignatureChecker;
use crate::{EcdsaSignature, SchnorrSignature, ScriptExecutionData, SigVersion, VerificationFlags};
use bitcoin::{PublicKey, Script};

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum SigError {
    #[error("generating key from slice: {0:?}")]
    FromSlice(bitcoin::key::FromSliceError),
    #[error("secp256k1 error: {0:?}")]
    Secp256k1(bitcoin::secp256k1::Error),
    #[error("ecdsa error: {0:?}")]
    Ecdsa(bitcoin::ecdsa::Error),
    #[error("")]
    SignatureDer,
    #[error("")]
    SignatureHashtype,
    #[error("")]
    BadPubkey,
}

pub fn eval_checksig(
    sig: &[u8],
    pubkey: &[u8],
    begincode: usize,
    exec_data: &ScriptExecutionData,
    flags: &VerificationFlags,
    checker: &dyn SignatureChecker,
    sig_version: SigVersion,
) -> Result<bool, SigError> {
    match sig_version {
        SigVersion::Base | SigVersion::WitnessV0 => {
            eval_checksig_pre_tapscript(sig, pubkey, sig_version, exec_data, flags, checker)
        }
        SigVersion::Taproot => {
            eval_checksig_tapscript(sig, pubkey, sig_version, exec_data, flags, checker)
        }
        SigVersion::Tapscript => unreachable!("key path in Taproot has no script"),
    }
}

fn eval_checksig_pre_tapscript(
    sig: &[u8],
    pubkey: &[u8],
    sig_version: SigVersion,
    exec_data: &ScriptExecutionData,
    flags: &VerificationFlags,
    checker: &dyn SignatureChecker,
) -> Result<bool, SigError> {
    let sig = EcdsaSignature::from_slice(sig).map_err(SigError::Ecdsa)?;
    let pubkey = PublicKey::from_slice(pubkey).map_err(SigError::FromSlice)?;

    let script_code = Script::from_bytes(b"todo: proper script");
    checker.check_ecdsa_signature(&sig, &pubkey, script_code, sig_version)?;

    Ok(true)
}

fn check_signature_encoding(sig: &[u8], flags: &VerificationFlags) -> Result<(), SigError> {
    // Empty signature. Not strictly DER encoded, but allowed to provide a
    // compact way to provide an invalid signature for use with CHECK(MULTI)SIG
    if sig.is_empty() {
        return Ok(());
    }

    // TODO:
    // if flags.contains(
    // VerificationFlags::DERSIG | VerificationFlags::LOW_S | VerificationFlags::STRICTENC,
    // ) && !is_valid_signature_encoding(sig)
    // {
    // return Err(SigError::SignatureDer);
    // }

    // if flags.contains(VerificationFlags::LOW_S) {
    // is_low_der_signature(sig)?;
    // }

    // if flags.contains(VerificationFlags::STRICTENC) && !is_defined_hashtype_signature(version, sig)
    // {
    // return Err(SigError::SignatureHashtype);
    // }

    Ok(())
}

fn check_pubkey_encoding(pubkey: &[u8], flags: &VerificationFlags) -> Result<(), SigError> {
    if flags.contains(VerificationFlags::STRICTENC) && !is_public_key(pubkey) {
        return Err(SigError::BadPubkey);
    }

    Ok(())
}

fn is_public_key(v: &[u8]) -> bool {
    match v.len() {
        33 if v[0] == 2 || v[0] == 3 => true,
        65 if v[0] == 4 => true,
        _ => false,
    }
}

fn eval_checksig_tapscript(
    sig: &[u8],
    pubkey: &[u8],
    sig_version: SigVersion,
    exec_data: &ScriptExecutionData,
    flags: &VerificationFlags,
    checker: &dyn SignatureChecker,
) -> Result<bool, SigError> {
    let sig = SchnorrSignature::from_slice(sig).map_err(SigError::Secp256k1)?;
    let pubkey = PublicKey::from_slice(pubkey).map_err(SigError::FromSlice)?;

    checker.check_schnorr_signature(&sig, &pubkey, sig_version, exec_data)?;

    Ok(true)
}
