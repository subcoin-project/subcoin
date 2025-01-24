use super::sig::{check_pubkey_encoding, check_signature_encoding};
use crate::interpreter::constants::{MAX_OPS_PER_SCRIPT, MAX_PUBKEYS_PER_MULTISIG};
use crate::signature_checker::SignatureChecker;
use crate::stack::{Stack, StackError};
use crate::{EcdsaSignature, SigVersion, VerificationFlags};
use bitcoin::script::PushBytesBuf;
use bitcoin::{PublicKey, Script};

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum MultiSigError {
    #[error("Invalid number of pubkeys, expected in the range of [0, {MAX_PUBKEYS_PER_MULTISIG}]")]
    InvalidPubkeyCount,
    #[error("Exceeded max OPS limit {MAX_OPS_PER_SCRIPT}")]
    TooManyOps,
    #[error("Invalid number of signatures, expected in the range of [0, {0}]")]
    InvalidSignatureCount(usize),
    #[error("multisig dummy argument has length {0} instead of 0")]
    SignatureNullDummy(usize),
    #[error("OP_CHECKMULTISIG and OP_CHECKMULTISIGVERIFY are disabled during tapscript execution")]
    TapscriptCheckMultiSig,
    #[error("")]
    CheckSigVerify,
    #[error(transparent)]
    Stack(#[from] StackError),
    #[error(transparent)]
    Signature(#[from] super::sig::SigError),
    #[error("ECDSA error: {0:?}")]
    Ecdsa(bitcoin::ecdsa::Error),
    #[error("Secp256k1 error: {0:?}")]
    Secp256k1(bitcoin::secp256k1::Error),
    #[error("generating key from slice: {0:?}")]
    FromSlice(bitcoin::key::FromSliceError),
}

pub enum MultiSigOp {
    CheckMultiSig,
    CheckMultiSigVerify,
}

pub fn execute_checkmultisig(
    stack: &mut Stack,
    flags: &VerificationFlags,
    begincode: usize,
    script: &Script,
    sig_version: SigVersion,
    checker: &dyn SignatureChecker,
    multisig_op: MultiSigOp,
    op_count: &mut usize,
) -> Result<(), MultiSigError> {
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
            return Err(MultiSigError::CheckSigVerify);
        }
        _ => {}
    }

    Ok(())
}

fn eval_checkmultisig(
    stack: &mut Stack,
    flags: &VerificationFlags,
    begincode: usize,
    script: &Script,
    sig_version: SigVersion,
    checker: &dyn SignatureChecker,
    op_count: &mut usize,
) -> Result<bool, MultiSigError> {
    if matches!(sig_version, SigVersion::Tapscript) {
        return Err(MultiSigError::TapscriptCheckMultiSig);
    }

    // ([sig ...] num_of_signatures [pubkey ...] num_of_pubkeys -- bool)

    let keys_count = stack.pop_num()?;
    if keys_count < 0.into() || keys_count > MAX_PUBKEYS_PER_MULTISIG.into() {
        return Err(MultiSigError::InvalidPubkeyCount);
    }

    let keys_count = keys_count.value() as usize;

    *op_count += keys_count;

    if *op_count > MAX_OPS_PER_SCRIPT {
        return Err(MultiSigError::TooManyOps);
    }

    let mut keys = Vec::with_capacity(keys_count);
    for _ in 0..keys_count {
        keys.push(stack.pop()?);
    }

    let sigs_count = stack.pop_num()?;
    let sigs_count = sigs_count.value() as usize;
    if sigs_count < 0 || sigs_count > keys_count {
        return Err(MultiSigError::InvalidSignatureCount(keys_count));
    }

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
    if flags.intersects(VerificationFlags::NULLDUMMY) && !dummy.is_empty() {
        return Err(MultiSigError::SignatureNullDummy(dummy.len()));
    }

    // Drop the signature in pre-segwit scripts but not segwit scripts
    let mut subscript = script.as_bytes()[begincode..].to_vec();

    for signature in &sigs {
        if matches!(sig_version, SigVersion::Base) {
            let mut push_bytes_sig = PushBytesBuf::new();
            push_bytes_sig
                .extend_from_slice(&signature)
                .expect("Failed to convert signature to PushBytes");
            let sig = bitcoin::script::Builder::default()
                .push_slice(push_bytes_sig)
                .into_script();
            super::sig::find_and_delete(&mut subscript, sig.as_bytes());
        }
    }

    let subscript = Script::from_bytes(&subscript);

    let mut success = true;
    let mut k = 0;
    let mut s = 0;
    while s < sigs.len() && success {
        let key = &keys[k];
        let sig = &sigs[s];

        check_signature_encoding(sig, flags)?;
        check_pubkey_encoding(key, flags, sig_version)?;

        let sig = EcdsaSignature::from_slice(&sig).map_err(MultiSigError::Ecdsa)?;
        let key = PublicKey::from_slice(&key).map_err(MultiSigError::FromSlice)?;

        if checker
            .check_ecdsa_signature(&sig, &key, &subscript, sig_version)
            .map_err(MultiSigError::Secp256k1)?
        {
            s += 1;
        }

        k += 1;

        success = sigs.len() - s <= keys.len() - k;
    }

    Ok(success)
}
