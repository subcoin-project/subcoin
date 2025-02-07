mod eval;

pub use self::eval::{eval_script, CheckMultiSigError, CheckSigError};

use crate::constants::{
    MAX_SCRIPT_ELEMENT_SIZE, MAX_STACK_SIZE, VALIDATION_WEIGHT_OFFSET, WITNESS_V0_KEYHASH_SIZE,
    WITNESS_V0_SCRIPTHASH_SIZE, WITNESS_V0_TAPROOT_SIZE,
};
use crate::error::Error;
use crate::signature_checker::{SignatureChecker, SECP};
use crate::stack::Stack;
use crate::{SchnorrSignature, ScriptExecutionData, SigVersion, VerifyFlags};
use bitcoin::hashes::Hash;
use bitcoin::opcodes::all::{OP_CHECKSIG, OP_DUP, OP_EQUALVERIFY, OP_HASH160};
use bitcoin::script::{Builder, Instruction, PushBytesBuf};
use bitcoin::taproot::{
    ControlBlock, LeafVersion, TAPROOT_ANNEX_PREFIX, TAPROOT_CONTROL_BASE_SIZE,
    TAPROOT_CONTROL_MAX_SIZE, TAPROOT_CONTROL_NODE_SIZE, TAPROOT_LEAF_MASK, TAPROOT_LEAF_TAPSCRIPT,
};
use bitcoin::{Script, TapLeafHash, Witness, WitnessProgram, WitnessVersion, XOnlyPublicKey};

/// Verifies the script validity.
///
/// - Ok(()): `return true` in C++.
/// - Err(err): `return false` with `serror` set.
pub fn verify_script<SC: SignatureChecker>(
    script_sig: &Script,
    script_pubkey: &Script,
    witness: &Witness,
    flags: &VerifyFlags,
    checker: &mut SC,
) -> Result<(), Error> {
    if flags.intersects(VerifyFlags::SIGPUSHONLY) && !script_sig.is_push_only() {
        return Err(Error::SigPushOnly);
    }

    // scriptSig and scriptPubKey must be evaluated sequentially on the same stack rather
    // than being simply concatnated (see CVE-2010-5141).
    let mut stack = Stack::with_flags(flags);

    eval_script(
        &mut stack,
        script_sig,
        &flags,
        checker,
        SigVersion::Base,
        &mut ScriptExecutionData::default(),
    )?;

    let stack_copy_for_p2sh = if flags.intersects(VerifyFlags::P2SH) {
        Some(stack.clone())
    } else {
        None
    };

    eval_script(
        &mut stack,
        script_pubkey,
        flags,
        checker,
        SigVersion::Base,
        &mut ScriptExecutionData::default(),
    )?;

    if stack.is_empty() {
        tracing::debug!("[verify_script] Invalid script: empty stack");
        return Err(Error::EvalFalse);
    }

    if !stack.peek_bool()? {
        tracing::debug!("[verify_script] Invalid script: false stack element");
        return Err(Error::EvalFalse);
    }

    let mut had_witness = false;

    // Bare witness program
    if flags.intersects(VerifyFlags::WITNESS) {
        if let Some(witness_program) = parse_witness_program(script_pubkey) {
            had_witness = true;

            // script_sig must be empty for all native witness programs, otherwise
            // we introduce malleability.
            if !script_sig.is_empty() {
                return Err(Error::WitnessMalleated);
            }

            verify_witness_program(witness, witness_program, flags, checker, false)?;

            // Bypass the cleanstack check at the end. The actual stack is obviously not clean
            // for witness programs.
            stack.truncate(1);
        }
    }

    // Additional validation for spend-to-script-hash transactions:
    match stack_copy_for_p2sh {
        Some(mut stack_copy) if script_pubkey.is_p2sh() => {
            // scriptSig must be literals-only or validation fails.
            if !script_sig.is_push_only() {
                return Err(Error::SigPushOnly);
            }

            // Restore stack.
            std::mem::swap(&mut stack, &mut stack_copy);

            // stack cannot be empty here, because if it was the
            // P2SH  HASH <> EQUAL  scriptPubKey would be evaluated with
            // an empty stack and the EvalScript above would return false.
            assert!(!stack.is_empty());

            let pubkey_serialized = stack.pop()?;
            let pubkey2 = Script::from_bytes(&pubkey_serialized);

            eval_script(
                &mut stack,
                &pubkey2,
                flags,
                checker,
                SigVersion::Base,
                &mut ScriptExecutionData::default(),
            )?;

            if stack.is_empty() {
                return Err(Error::EvalFalse);
            }

            if !stack.peek_bool()? {
                return Err(Error::EvalFalse);
            }

            // P2SH witness program
            if flags.intersects(VerifyFlags::WITNESS) {
                if let Some(witness_program) = parse_witness_program(pubkey2) {
                    had_witness = true;
                    let mut push_bytes = PushBytesBuf::new();
                    push_bytes
                        .extend_from_slice(pubkey2.as_bytes())
                        .expect("Failed to convert pubkey to PushBytes");
                    let redeem_script = Builder::default().push_slice(push_bytes).into_script();

                    if script_sig != &redeem_script {
                        // The scriptSig must be _exactly_ a single push of the redeemScript. Otherwise
                        // we reintroduce malleability.
                        return Err(Error::WitnessMalleatedP2SH);
                    }

                    verify_witness_program(witness, witness_program, &flags, checker, true)?;

                    // Bypass the cleanstack check at the end. The actual stack is obviously not clean
                    // for witness programs.
                    stack.truncate(1);
                }
            }
        }
        _ => {}
    }

    // The CLEANSTACK check is only performed after potential P2SH evaluation,
    // as the non-P2SH evaluation of a P2SH script will obviously not result in
    // a clean stack (the P2SH inputs remain). The same holds for witness evaluation.
    if flags.intersects(VerifyFlags::CLEANSTACK) {
        // Disallow CLEANSTACK without P2SH, as otherwise a switch CLEANSTACK->P2SH+CLEANSTACK
        // would be possible, which is not a softfork (and P2SH should be one).
        assert!(
            flags.verify_p2sh() && flags.verify_witness(),
            "Disallow CLEANSTACK without P2SH"
        );
        if stack.len() != 1 {
            return Err(Error::CleanStack);
        }
    }

    if flags.intersects(VerifyFlags::WITNESS) {
        // We can't check for correct unexpected witness data if P2SH was off, so require
        // that WITNESS implies P2SH. Otherwise, going from WITNESS->P2SH+WITNESS would be
        // possible, which is not a softfork.
        assert!(flags.verify_p2sh());
        if !had_witness && !witness.is_empty() {
            return Err(Error::WitnessUnexpected);
        }
    }

    // Only return Ok(()) at the end after all checks pass.
    Ok(())
}

fn parse_witness_program(script_pubkey: &Script) -> Option<WitnessProgram> {
    script_pubkey.witness_version().and_then(|witness_version| {
        WitnessProgram::new(witness_version, &script_pubkey.as_bytes()[2..]).ok()
    })
}

fn verify_witness_program(
    witness: &Witness,
    witness_program: WitnessProgram,
    flags: &VerifyFlags,
    checker: &mut impl SignatureChecker,
    is_p2sh: bool,
) -> Result<(), Error> {
    // TODO: since we clone the entire witness data, we use stack.pop() later instead of
    // SpanPopBack(stack) in Bitcoin Core. Perhaps avoid this allocation later.
    let mut witness_stack = Stack::with_data(witness.to_vec());

    let witness_version = witness_program.version();
    let program = witness_program.program();

    if witness_version == WitnessVersion::V0 {
        let program_size = program.len();

        if program_size == WITNESS_V0_SCRIPTHASH_SIZE {
            // BIP141 P2WSH: 32-byte witness v0 program (which encodes SHA256(script))
            if witness_stack.is_empty() {
                return Err(Error::WitnessProgramWitnessEmpty);
            }

            let witness_script = witness_stack.pop()?;
            let exec_script = Script::from_bytes(&witness_script);

            let exec_script_hash: [u8; 32] = exec_script.wscript_hash().to_byte_array();

            if exec_script_hash.as_slice() != program.as_bytes() {
                return Err(Error::WitnessProgramMismatch);
            }

            execute_witness_script(
                &witness_stack,
                &exec_script,
                flags,
                SigVersion::WitnessV0,
                checker,
                &mut ScriptExecutionData::default(),
            )?;
        } else if program_size == WITNESS_V0_KEYHASH_SIZE {
            // BIP141 P2WPKH: 20-byte witness v0 program (which encodes Hash160(pubkey))
            //
            // ScriptPubKey: 0 <20-byte-PublicKeyHash>
            // ScriptSig: (empty)
            // Witness: <Signature> <PublicKey>
            if witness_stack.len() != 2 {
                return Err(Error::WitnessProgramMismatch);
            }

            let exec_script = Builder::default()
                .push_opcode(OP_DUP)
                .push_opcode(OP_HASH160)
                .push_slice(program)
                .push_opcode(OP_EQUALVERIFY)
                .push_opcode(OP_CHECKSIG)
                .into_script();

            execute_witness_script(
                &witness_stack,
                &exec_script,
                flags,
                SigVersion::WitnessV0,
                checker,
                &mut ScriptExecutionData::default(),
            )?;
        } else {
            return Err(Error::WitnessProgramWrongLength);
        }
    } else if witness_version == WitnessVersion::V1
        && program.len() == WITNESS_V0_TAPROOT_SIZE
        && !is_p2sh
    {
        // BIP 341 Taproot: 32-byte non-P2SH witness v1 program (which encodes a P2C-tweaked pubkey)
        // https://github.com/bitcoin/bips/blob/master/bip-0341.mediawiki#script-validation-rules
        if !flags.intersects(VerifyFlags::TAPROOT) {
            return Ok(());
        }

        if witness_stack.is_empty() {
            return Err(Error::WitnessProgramWitnessEmpty);
        }

        let mut exec_data = if witness_stack.len() >= 2
            && !witness_stack.last()?.is_empty()
            && witness_stack.last()?[0] == TAPROOT_ANNEX_PREFIX
        {
            // Drop annex (this is non-standard; see IsWitnessStandard
            let annex = witness_stack.pop()?;
            ScriptExecutionData {
                annex_hash: bitcoin::hashes::sha256::Hash::hash(&annex),
                annex_present: true,
                annex: Some(annex),
                ..Default::default()
            }
        } else {
            ScriptExecutionData::default()
        };

        exec_data.annex_init = true;

        if witness_stack.len() == 1 {
            // Key path spending (stack size is 1 after removing optional annex).
            let sig = witness_stack.last()?;
            let sig = SchnorrSignature::from_slice(&sig).map_err(Error::SchnorrSignature)?;
            let pubkey =
                XOnlyPublicKey::from_slice(program.as_bytes()).map_err(Error::Secp256k1)?;
            checker.check_schnorr_signature(&sig, &pubkey, SigVersion::Taproot, &exec_data)?;
        } else {
            // Script path spending (stack size is >1 after removing optional annex).
            let control = witness_stack.pop()?;
            let script = witness_stack.pop()?;

            if control.len() < TAPROOT_CONTROL_BASE_SIZE
                || control.len() > TAPROOT_CONTROL_MAX_SIZE
                || ((control.len() - TAPROOT_CONTROL_BASE_SIZE) % TAPROOT_CONTROL_NODE_SIZE != 0)
            {
                return Err(Error::TaprootWrongControlSize);
            }

            let script = Script::from_bytes(&script);

            let leaf_version = control[0] & TAPROOT_LEAF_MASK;

            // ComputeTapleafHash
            exec_data.tapleaf_hash = TapLeafHash::from_script(
                script,
                LeafVersion::from_consensus(leaf_version).expect("Failed to compute leaf version"),
            );

            // VerifyTaprootCommitment
            let control_block = ControlBlock::decode(&control).map_err(Error::Taproot)?;
            let output_key =
                XOnlyPublicKey::from_slice(program.as_bytes()).map_err(Error::Secp256k1)?;
            if !control_block.verify_taproot_commitment(&SECP, output_key, script) {
                return Err(Error::WitnessProgramMismatch);
            }
            exec_data.tapleaf_hash_init = true;

            if leaf_version == TAPROOT_LEAF_TAPSCRIPT {
                // Tapscript (leaf version 0xc0)
                exec_data.validation_weight_left = witness.size() as i64 + VALIDATION_WEIGHT_OFFSET;
                exec_data.validation_weight_left_init = true;
                let exec_script = script;
                return execute_witness_script(
                    &witness_stack,
                    exec_script,
                    flags,
                    SigVersion::Tapscript,
                    checker,
                    &mut exec_data,
                );
            }

            if flags.intersects(VerifyFlags::DISCOURAGE_UPGRADABLE_TAPROOT_VERSION) {
                return Err(Error::DiscourageUpgradableTaprootVersion);
            }
        }
    } else if flags.intersects(VerifyFlags::DISCOURAGE_UPGRADABLE_WITNESS_PROGRAM) {
        return Err(Error::DiscourageUpgradableWitnessProgram);
    }

    // Other version/size/p2sh combinations returns true for future softfork compatibility.
    // Ok(true)?
    Ok(())
}

fn execute_witness_script(
    stack_span: &[Vec<u8>],
    exec_script: &Script,
    flags: &VerifyFlags,
    sig_version: SigVersion,
    checker: &mut impl SignatureChecker,
    exec_data: &mut ScriptExecutionData,
) -> Result<(), Error> {
    let mut stack = Stack::new(stack_span.to_vec(), true);

    if sig_version == SigVersion::Tapscript {
        // OP_SUCCESSx processing overrides everything, including stack element size limits
        for instruction in exec_script.instructions() {
            match instruction.map_err(Error::ReadInstruction)? {
                Instruction::Op(opcode) => {
                    // New opcodes will be listed here. May use a different sigversion to modify existing opcodes.
                    if is_op_success(opcode.to_u8()) {
                        if flags.intersects(VerifyFlags::DISCOURAGE_OP_SUCCESS) {
                            return Err(Error::DiscourageOpSuccess);
                        }
                        return Ok(());
                    }
                }
                Instruction::PushBytes(_) => return Err(Error::BadOpcode),
            }
        }

        // Tapscript enforces initial stack size limits (altstack is empty here)
        if stack.len() > MAX_STACK_SIZE {
            return Err(Error::StackSize);
        }
    }

    // Disallow stack item size > MAX_SCRIPT_ELEMENT_SIZE in witness stack
    if stack
        .iter()
        .any(|elem| elem.len() > MAX_SCRIPT_ELEMENT_SIZE)
    {
        return Err(Error::PushSize);
    }

    // Run the script interpreter.
    eval_script(
        &mut stack,
        exec_script,
        flags,
        checker,
        sig_version,
        exec_data,
    )?;

    // Scripts inside witness implicitly require cleanstack behavior
    if stack.len() != 1 {
        return Err(Error::CleanStack);
    }

    if !stack.peek_bool()? {
        return Err(Error::EvalFalse);
    }

    Ok(())
}

fn is_op_success(opcode: u8) -> bool {
    (opcode == 0x50) || (opcode >= 0x7b && opcode <= 0xb9)
}
