use super::sig::{check_pubkey_encoding, check_signature_encoding, CheckSigError};
use crate::constants::{MAX_OPS_PER_SCRIPT, MAX_PUBKEYS_PER_MULTISIG};
use crate::signature_checker::{SignatureChecker, SignatureError};
use crate::stack::{Stack, StackError};
use crate::{EcdsaSignature, SigVersion, VerifyFlags};
use bitcoin::script::PushBytesBuf;
use bitcoin::{PublicKey, Script};

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum CheckMultiSigError {
    #[error("Invalid number of pubkeys, expected in the range of [0, {MAX_PUBKEYS_PER_MULTISIG}]")]
    InvalidPubkeyCount,
    #[error("Exceeded max OPS limit {MAX_OPS_PER_SCRIPT}")]
    TooManyOps,
    #[error("Invalid number of signatures, expected in the range of [0, {0}]")]
    InvalidSignatureCount(usize),
    #[error("Multisig dummy argument has length {0} instead of 0")]
    SignatureNullDummy(usize),
    #[error("OP_CHECKMULTISIG and OP_CHECKMULTISIGVERIFY are disabled during tapscript execution")]
    TapscriptCheckMultiSig,
    #[error("CHECK_MULTISIGVERIFY failed")]
    CheckMultiSigVerify,
    #[error("ECDSA error: {0:?}")]
    Ecdsa(bitcoin::ecdsa::Error),
    #[error("Generating key from slice: {0:?}")]
    FromSlice(bitcoin::key::FromSliceError),
    #[error("Secp256k1 error: {0:?}")]
    Secp256k1(bitcoin::secp256k1::Error),
    #[error(transparent)]
    CheckSig(#[from] CheckSigError),
    #[error(transparent)]
    Signature(#[from] SignatureError),
    #[error(transparent)]
    Stack(#[from] StackError),
}

pub enum MultiSigOp {
    CheckMultiSig,
    CheckMultiSigVerify,
}

/// Handles the OP_CHECKMULTISIG opcode.
///
/// This opcode validates a multi-signature script against the provided public keys and signatures.
pub(super) fn handle_checkmultisig(
    stack: &mut Stack,
    flags: &VerifyFlags,
    begincode: usize,
    script: &Script,
    sig_version: SigVersion,
    checker: &mut impl SignatureChecker,
    multisig_op: MultiSigOp,
    op_count: &mut usize,
) -> Result<(), CheckMultiSigError> {
    let success = eval_checkmultisig(
        stack,
        flags,
        begincode,
        script,
        sig_version,
        checker,
        op_count,
    )?;

    match multisig_op {
        MultiSigOp::CheckMultiSig => {
            stack.push_bool(success);
        }
        MultiSigOp::CheckMultiSigVerify if !success => {
            return Err(CheckMultiSigError::CheckMultiSigVerify);
        }
        _ => {}
    }

    Ok(())
}

fn eval_checkmultisig(
    stack: &mut Stack,
    flags: &VerifyFlags,
    begincode: usize,
    script: &Script,
    sig_version: SigVersion,
    checker: &mut impl SignatureChecker,
    op_count: &mut usize,
) -> Result<bool, CheckMultiSigError> {
    if matches!(sig_version, SigVersion::Tapscript) {
        return Err(CheckMultiSigError::TapscriptCheckMultiSig);
    }

    // ([sig ...] num_of_signatures [pubkey ...] num_of_pubkeys -- bool)

    let keys_count = stack.pop_num()?;
    if keys_count < 0.into() || keys_count > MAX_PUBKEYS_PER_MULTISIG.into() {
        return Err(CheckMultiSigError::InvalidPubkeyCount);
    }

    let keys_count = keys_count.value() as usize;

    *op_count += keys_count;

    if *op_count > MAX_OPS_PER_SCRIPT {
        return Err(CheckMultiSigError::TooManyOps);
    }

    let mut keys = Vec::with_capacity(keys_count);
    for _ in 0..keys_count {
        keys.push(stack.pop()?);
    }

    let sigs_count = stack.pop_num()?.value();
    if sigs_count < 0 || sigs_count as usize > keys_count {
        return Err(CheckMultiSigError::InvalidSignatureCount(keys_count));
    }

    let sigs_count = sigs_count as usize;
    let mut sigs = Vec::with_capacity(sigs_count);
    for _ in 0..sigs_count {
        sigs.push(stack.pop()?);
    }

    // A bug in the original Satoshi client implementation means one more
    // stack value than should be used must be popped.  Unfortunately, this
    // buggy behavior is now part of the consensus and a hard fork would be
    // required to fix it.
    let dummy = stack.pop()?;

    // Since the dummy argument is otherwise not checked, it could be any
    // value which unfortunately provides a source of malleability.  Thus,
    // there is a script flag to force an error when the value is NOT 0.
    if flags.intersects(VerifyFlags::NULLDUMMY) && !dummy.is_empty() {
        return Err(CheckMultiSigError::SignatureNullDummy(dummy.len()));
    }

    // Drop the signature in pre-segwit scripts but not segwit scripts
    let mut subscript = script.as_bytes()[begincode..].to_vec();

    for signature in &sigs {
        if matches!(sig_version, SigVersion::Base) {
            let mut push_buf = PushBytesBuf::new();
            push_buf
                .extend_from_slice(&signature)
                .expect("Signature within length limits");
            let sig_script = bitcoin::script::Builder::default()
                .push_slice(push_buf)
                .into_script();
            let found = super::sig::find_and_delete(&mut subscript, sig_script.as_bytes());
            if found > 0 && flags.intersects(VerifyFlags::CONST_SCRIPTCODE) {
                return Err(CheckSigError::FindAndDelete.into());
            }
        }
    }

    let subscript = Script::from_bytes(&subscript);

    // Verify signatures against public keys.
    let mut success = true;
    let mut key_idx = 0;
    let mut sig_idx = 0;
    while sig_idx < sigs.len() && success {
        let key = &keys[key_idx];
        let sig = &sigs[sig_idx];

        check_signature_encoding(sig, flags)?;
        check_pubkey_encoding(key, flags, sig_version)?;

        let sig = EcdsaSignature::parse_der_lax(&sig).map_err(CheckMultiSigError::Ecdsa)?;
        let key = PublicKey::from_slice(&key).map_err(CheckMultiSigError::FromSlice)?;

        if checker
            .check_ecdsa_signature(&sig, &key, &subscript, sig_version)
            .map_err(CheckMultiSigError::Signature)?
        {
            sig_idx += 1;
        }
        key_idx += 1;

        // Early exit if remaining keys can't satisfy remaining signatures.
        success = sigs.len() - sig_idx <= keys.len() - key_idx;
    }

    Ok(success)
}
