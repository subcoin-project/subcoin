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
    FindAndDelete,
    #[error("")]
    SignatureDer,
    #[error("")]
    SignatureHashtype,
    #[error("")]
    BadPubkey,
    #[error("")]
    WitnessPubkeyType,
    #[error("")]
    NullFail,
}

pub fn eval_checksig(
    sig: &[u8],
    pubkey: &[u8],
    script: &Script,
    begincodehash: usize,
    exec_data: &ScriptExecutionData,
    flags: &VerificationFlags,
    checker: &dyn SignatureChecker,
    sig_version: SigVersion,
) -> Result<bool, SigError> {
    match sig_version {
        SigVersion::Base | SigVersion::WitnessV0 => eval_checksig_pre_tapscript(
            sig,
            pubkey,
            script,
            begincodehash,
            flags,
            checker,
            sig_version,
        ),
        SigVersion::Taproot => eval_checksig_tapscript(
            sig,
            pubkey,
            begincodehash,
            flags,
            checker,
            sig_version,
            exec_data,
        ),
        SigVersion::Tapscript => unreachable!("key path in Taproot has no script"),
    }
}

fn eval_checksig_pre_tapscript(
    sig: &[u8],
    pubkey: &[u8],
    script: &Script,
    begincodehash: usize,
    flags: &VerificationFlags,
    checker: &dyn SignatureChecker,
    sig_version: SigVersion,
) -> Result<bool, SigError> {
    let mut subscript = script.as_bytes()[begincodehash..].to_vec();

    if matches!(sig_version, SigVersion::Base) {
        let found = find_and_delete(&mut subscript, sig);

        if found > 0 && flags.contains(VerificationFlags::CONST_SCRIPTCODE) {
            return Err(SigError::FindAndDelete);
        }
    }

    check_signature_encoding(sig, flags)?;
    check_pubkey_encoding(pubkey, flags, sig_version)?;

    let signature = EcdsaSignature::from_slice(sig).map_err(SigError::Ecdsa)?;
    let pubkey = PublicKey::from_slice(pubkey).map_err(SigError::FromSlice)?;

    let script_code = Script::from_bytes(&subscript);

    let success = checker.check_ecdsa_signature(&signature, &pubkey, script_code, sig_version)?;

    if !success && flags.contains(VerificationFlags::NULLFAIL) && !sig.is_empty() {
        return Err(SigError::NullFail);
    }

    Ok(true)
}

fn find_and_delete(script: &mut Vec<u8>, sig: &[u8]) -> usize {
    if sig.is_empty() || sig.len() > script.len() {
        return 0;
    }

    let mut found = 0;
    let mut result = Vec::with_capacity(script.len());
    let mut i = 0;

    while i <= script.len() - sig.len() {
        // Check for a match directly by comparing byte by byte
        if &script[i..i + sig.len()] == sig {
            found += 1;
            i += sig.len(); // Skip the matched signature
        } else {
            result.push(script[i]); // Keep the current byte if no match
            i += 1;
        }
    }

    // Append any remaining bytes after the last match
    result.extend_from_slice(&script[i..]);

    // Replace the original script with the modified one
    *script = result;

    found
}

fn check_signature_encoding(sig: &[u8], flags: &VerificationFlags) -> Result<(), SigError> {
    // Empty signature. Not strictly DER encoded, but allowed to provide a
    // compact way to provide an invalid signature for use with CHECK(MULTI)SIG
    if sig.is_empty() {
        return Ok(());
    }

    if flags.contains(
        VerificationFlags::DERSIG | VerificationFlags::LOW_S | VerificationFlags::STRICTENC,
    ) && !is_valid_signature_encoding(sig)
    {
        return Err(SigError::SignatureDer);
    }

    // TODO:
    // if flags.contains(VerificationFlags::LOW_S) {
    // is_low_der_signature(sig)?;
    // }

    if flags.contains(VerificationFlags::STRICTENC) && !is_defined_hashtype_signature(sig) {
        return Err(SigError::SignatureHashtype);
    }

    Ok(())
}

fn is_valid_signature_encoding(sig: &[u8]) -> bool {
    // Minimum and maximum size constraints
    if sig.len() < 9 || sig.len() > 73 {
        return false;
    }

    // A signature is of type 0x30 (compound)
    if sig[0] != 0x30 {
        return false;
    }

    // Make sure the length covers the entire signature
    if sig[1] as usize != sig.len() - 3 {
        return false;
    }

    // Extract the length of the R element
    let len_r = sig[3] as usize;

    // Make sure the length of the S element is still inside the signature
    if 5 + len_r >= sig.len() {
        return false;
    }

    // Extract the length of the S element
    let len_s = sig[5 + len_r] as usize;

    // Verify that the length of the signature matches the sum of the length of the elements
    if len_r + len_s + 7 != sig.len() {
        return false;
    }

    // Check whether the R element is an integer
    if sig[2] != 0x02 {
        return false;
    }

    // Zero-length integers are not allowed for R
    if len_r == 0 {
        return false;
    }

    // Negative numbers are not allowed for R
    if sig[4] & 0x80 != 0 {
        return false;
    }

    // Null bytes at the start of R are not allowed, unless R would otherwise be interpreted as a negative number
    if len_r > 1 && sig[4] == 0x00 && sig[5] & 0x80 == 0 {
        return false;
    }

    // Check whether the S element is an integer
    if sig[len_r + 4] != 0x02 {
        return false;
    }

    // Zero-length integers are not allowed for S
    if len_s == 0 {
        return false;
    }

    // Negative numbers are not allowed for S
    if sig[len_r + 6] & 0x80 != 0 {
        return false;
    }

    // Null bytes at the start of S are not allowed, unless S would otherwise be interpreted as a negative number
    if len_s > 1 && sig[len_r + 6] == 0x00 && sig[len_r + 7] & 0x80 == 0 {
        return false;
    }

    true
}

fn is_low_der_signature(sig: &[u8]) -> bool {
    if !is_valid_signature_encoding(sig) {
        return false;
    }

    todo!("check low s")
}

// Constants for hash type
const SIGHASH_ALL: u8 = 0x01;
const SIGHASH_NONE: u8 = 0x02;
const SIGHASH_SINGLE: u8 = 0x03;
const SIGHASH_ANYONECANPAY: u8 = 0x80;

// Constant for compressed public key size
const COMPRESSED_PUBKEY_SIZE: usize = 33;

fn is_defined_hashtype_signature(sig: &[u8]) -> bool {
    if sig.is_empty() {
        return false;
    }

    // Extract the hash type byte from the signature
    let n_hash_type = sig[sig.len() - 1] & !SIGHASH_ANYONECANPAY;

    // Check if the hash type is within the valid range
    n_hash_type >= SIGHASH_ALL && n_hash_type <= SIGHASH_SINGLE
}

fn check_pubkey_encoding(
    pubkey: &[u8],
    flags: &VerificationFlags,
    sig_version: SigVersion,
) -> Result<(), SigError> {
    if flags.contains(VerificationFlags::STRICTENC) && !is_public_key(pubkey) {
        return Err(SigError::BadPubkey);
    }

    if flags.contains(VerificationFlags::WITNESS_PUBKEYTYPE)
        && matches!(sig_version, SigVersion::WitnessV0)
        && !is_compressed_pubkey(pubkey)
    {
        return Err(SigError::WitnessPubkeyType);
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

fn is_compressed_pubkey(pubkey: &[u8]) -> bool {
    // Check if the public key is the correct size for a compressed key
    if pubkey.len() != COMPRESSED_PUBKEY_SIZE {
        return false;
    }

    // Check if the public key has a valid prefix for a compressed key
    matches!(pubkey[0], 0x02 | 0x03)
}

fn eval_checksig_tapscript(
    sig: &[u8],
    pubkey: &[u8],
    begincodehash: usize,
    flags: &VerificationFlags,
    checker: &dyn SignatureChecker,
    sig_version: SigVersion,
    exec_data: &ScriptExecutionData,
) -> Result<bool, SigError> {
    let sig = SchnorrSignature::from_slice(sig).map_err(SigError::Secp256k1)?;
    let pubkey = PublicKey::from_slice(pubkey).map_err(SigError::FromSlice)?;

    checker.check_schnorr_signature(&sig, &pubkey, sig_version, exec_data)?;

    Ok(true)
}
