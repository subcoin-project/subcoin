use super::sig::{CheckSigError, check_pubkey_encoding, check_signature_encoding, find_and_delete};
use crate::constants::{MAX_OPS_PER_SCRIPT, MAX_PUBKEYS_PER_MULTISIG};
use crate::signature_checker::{SignatureChecker, SignatureError, check_ecdsa_signature};
use crate::stack::{Stack, StackError};
use crate::{SigVersion, VerifyFlags};
use bitcoin::Script;
use bitcoin::script::{Builder, PushBytesBuf};

/// Multisig error type.
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
    #[error(transparent)]
    CheckSig(#[from] CheckSigError),
    #[error(transparent)]
    InvalidSignature(#[from] SignatureError),
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
#[allow(clippy::too_many_arguments)]
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
            let mut push_buf = PushBytesBuf::with_capacity(signature.len());
            push_buf
                .extend_from_slice(signature)
                .expect("Signature length must be within length limits; qed");
            let sig_script = Builder::default().push_slice(push_buf).into_script();
            let found = find_and_delete(&mut subscript, sig_script.as_bytes());
            if found > 0 && flags.intersects(VerifyFlags::CONST_SCRIPTCODE) {
                return Err(CheckSigError::FindAndDelete.into());
            }
        }
    }

    let subscript = Script::from_bytes(&subscript);

    // Verify signatures against public keys.
    let mut success = true;
    let mut checked_keys_count = 0;
    let mut satisfied_sigs_count = 0;

    while satisfied_sigs_count < sigs.len() && success {
        let key = &keys[checked_keys_count];
        let sig = &sigs[satisfied_sigs_count];

        check_signature_encoding(sig, flags)?;
        check_pubkey_encoding(key, flags, sig_version)?;

        let signature_is_correct = check_ecdsa_signature(sig, key, checker, subscript, sig_version);

        if signature_is_correct {
            satisfied_sigs_count += 1;
        }

        checked_keys_count += 1;

        // Early exit if remaining keys can't satisfy remaining signatures.
        success = keys.len() - checked_keys_count >= sigs.len() - satisfied_sigs_count;
    }

    Ok(success)
}
