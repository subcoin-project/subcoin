use crate::signature_checker::SignatureChecker;
use crate::stack::{Stack, StackError};
use crate::{SigVersion, VerificationFlags};
use bitcoin::Script;

/// Maximum number of public keys per multisig
pub const MAX_PUBKEYS_PER_MULTISIG: i64 = 20;

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum MultisigError {
    #[error("")]
    PubkeyCount,
    #[error("")]
    SigCount,
    #[error("")]
    BadPubkey,
    #[error("")]
    SignatureNullDummy,
    #[error(transparent)]
    Stack(#[from] StackError),
}

pub enum Operation {
    CheckMultisig,
    CheckMultisigVerify,
}

pub fn execute_checkmultisig(
    stack: &mut Stack,
    flags: &VerificationFlags,
    begincode: usize,
    script: &Script,
    version: SigVersion,
    checker: &dyn SignatureChecker,
    operation: Operation,
) -> Result<(), MultisigError> {
    let success = eval_checkmultisig(stack, flags, begincode, script, version, checker)?;
    Ok(())
}

fn eval_checkmultisig(
    stack: &mut Stack,
    flags: &VerificationFlags,
    begincode: usize,
    script: &Script,
    version: SigVersion,
    checker: &dyn SignatureChecker,
) -> Result<bool, MultisigError> {
    let keys_count = stack.pop_num()?;
    if keys_count < 0.into() || keys_count > MAX_PUBKEYS_PER_MULTISIG.into() {
        return Err(MultisigError::PubkeyCount);
    }

    let keys_count = keys_count.value() as usize;
    let mut keys = Vec::with_capacity(keys_count);
    for _ in 0..keys_count {
        keys.push(stack.pop()?);
    }

    let sigs_count = stack.pop_num()?;
    if sigs_count < 0.into() || sigs_count.value() as usize > keys_count {
        return Err(MultisigError::SigCount);
    }

    let sigs_count = sigs_count.value() as usize;
    let mut sigs = Vec::with_capacity(sigs_count);
    for _ in 0..sigs_count {
        sigs.push(stack.pop()?);
    }

    /*
    let mut subscript = script.subscript(begincode);

    for signature in &sigs {
        let sighash = parse_hash_type(version, &signature);
        match version {
            SigVersion::WitnessV0 => (),
            SigVersion::Base => {
                let signature_script = Builder::default().push_data(&*signature).into_script();
                subscript = subscript.find_and_delete(&*signature_script);
            }
        }
    }
    */

    let mut success = true;
    let mut k = 0;
    let mut s = 0;
    while s < sigs.len() && success {
        let key = &keys[k];
        let sig = &sigs[s];

        // check_signature_encoding(sig, flags, version)?;
        // check_pubkey_encoding(key, flags)?;

        // if check_signature(checker, sig, key, &subscript, version) {
        // s += 1;
        // }

        k += 1;

        success = sigs.len() - s <= keys.len() - k;
    }

    if !stack.pop()?.is_empty() && flags.intersects(VerificationFlags::NULLDUMMY) {
        return Err(MultisigError::SignatureNullDummy);
    }

    Ok(true)
}
