use super::{eval_script, ScriptError};
use crate::constants::{MAX_SCRIPT_ELEMENT_SIZE, MAX_STACK_SIZE, WITNESS_V0_SCRIPTHASH_SIZE};
use crate::signature_checker::SignatureChecker;
use crate::stack::Stack;
use crate::{ScriptExecutionData, SigVersion, VerificationFlags};
use bitcoin::hashes::Hash;
use bitcoin::opcodes::all::{OP_CHECKSIG, OP_DUP, OP_EQUALVERIFY, OP_HASH160};
use bitcoin::script::{Builder, Instruction, PushBytesBuf};
use bitcoin::{Script, Witness, WitnessProgram, WitnessVersion};

fn parse_witness_program(script: &Script) -> Option<WitnessProgram> {
    script.witness_version().and_then(|witness_version| {
        WitnessProgram::new(witness_version, &script.as_bytes()[2..]).ok()
    })
}

/// - Ok(()): `return true` in C++.
/// - Err(err): `return false` with `serror` set.
pub fn verify_script(
    script_sig: &Script,
    script_pubkey: &Script,
    witness: &Witness,
    flags: VerificationFlags,
    checker: &mut impl SignatureChecker,
    sig_version: SigVersion,
) -> Result<(), ScriptError> {
    if flags.intersects(VerificationFlags::SIGPUSHONLY) && !script_sig.is_push_only() {
        return Err(ScriptError::SigPushOnly);
    }

    // scriptSig and scriptPubKey must be evaluated sequentially on the same stack rather
    // than being simply concatnated (see CVE-2010-5141).
    let mut stack = Stack::default();

    eval_script(
        &mut stack,
        script_sig,
        &flags,
        checker,
        SigVersion::Base,
        &mut ScriptExecutionData::default(),
    )?;

    let mut stack_copy = if flags.intersects(VerificationFlags::P2SH) {
        Some(stack.clone())
    } else {
        None
    };

    eval_script(
        &mut stack,
        script_pubkey,
        &flags,
        checker,
        SigVersion::Base,
        &mut ScriptExecutionData::default(),
    )?;

    if stack.is_empty() {
        return Err(ScriptError::EvalFalse);
    }

    if !stack.peek_bool()? {
        return Err(ScriptError::EvalFalse);
    }

    let mut had_witness = false;

    // Verify witness program
    if flags.verify_witness() {
        if let Some(witness_program) = parse_witness_program(script_pubkey) {
            if !script_sig.is_empty() {
                return Err(ScriptError::WitnessMalleated);
            }

            had_witness = true;

            verify_witness_program(witness, witness_program, &flags, checker, false)?;

            // Bypass the cleanstack check at the end. The actual stack is obviously not clean
            // for witness programs.
            stack.truncate(1);
        }
    }

    // Additional validation for spend-to-script-hash transactions:
    if flags.verify_p2sh() && script_pubkey.is_p2sh() {
        if !script_sig.is_push_only() {
            return Err(ScriptError::SigPushOnly);
        }

        // Restore stack.
        std::mem::swap(&mut stack, &mut stack_copy.take().unwrap());

        // stack cannot be empty here, because if it was the
        // P2SH  HASH <> EQUAL  scriptPubKey would be evaluated with
        // an empty stack and the EvalScript above would return false.
        assert!(!stack.is_empty());

        let elem = stack.pop()?;
        let pubkey2 = Script::from_bytes(&elem);

        eval_script(
            &mut stack,
            &pubkey2,
            &flags,
            checker,
            SigVersion::Base,
            &mut ScriptExecutionData::default(),
        )?;

        if stack.is_empty() {
            return Err(ScriptError::EvalFalse);
        }

        if !stack.peek_bool()? {
            return Err(ScriptError::EvalFalse);
        }

        // P2SH witness program
        if flags.intersects(VerificationFlags::WITNESS) {
            if let Some(witness_program) = parse_witness_program(pubkey2) {
                let mut push_bytes = PushBytesBuf::new();
                push_bytes
                    .extend_from_slice(pubkey2.as_bytes())
                    .expect("Failed to convert pubkey to PushBytes");
                let redeem_script = Builder::default().push_slice(push_bytes).into_script();

                if script_sig != &redeem_script {
                    // The scriptSig must be _exactly_ a single push of the redeemScript. Otherwise
                    // we reintroduce malleability.
                    return Err(ScriptError::WitnessMalleatedP2SH);
                }

                had_witness = true;

                verify_witness_program(witness, witness_program, &flags, checker, true)?;

                // Bypass the cleanstack check at the end. The actual stack is obviously not clean
                // for witness programs.
                stack.truncate(1);
            }
        }
    }

    // The CLEANSTACK check is only performed after potential P2SH evaluation,
    // as the non-P2SH evaluation of a P2SH script will obviously not result in
    // a clean stack (the P2SH inputs remain). The same holds for witness evaluation.
    if flags.intersects(VerificationFlags::CLEANSTACK) {
        // Disallow CLEANSTACK without P2SH, as otherwise a switch CLEANSTACK->P2SH+CLEANSTACK
        // would be possible, which is not a softfork (and P2SH should be one).
        assert!(
            flags.verify_p2sh() && flags.verify_witness(),
            "Disallow CLEANSTACK without P2SH"
        );
        if stack.len() != 1 {
            return Err(ScriptError::CleanStack);
        }
    }

    if flags.intersects(VerificationFlags::WITNESS) {
        // We can't check for correct unexpected witness data if P2SH was off, so require
        // that WITNESS implies P2SH. Otherwise, going from WITNESS->P2SH+WITNESS would be
        // possible, which is not a softfork.
        assert!(flags.verify_p2sh());
        if !had_witness && !witness.is_empty() {
            return Err(ScriptError::WitnessUnexpected);
        }
    }

    // Only return Ok(()) at the end after all checks pass.
    Ok(())
}

fn verify_witness_program(
    witness: &Witness,
    witness_program: WitnessProgram,
    flags: &VerificationFlags,
    checker: &mut impl SignatureChecker,
    is_p2sh: bool,
) -> Result<(), ScriptError> {
    let stack = Stack::with_data(witness.to_vec());

    let witness_version = witness_program.version();

    if witness_version == WitnessVersion::V0 {
        match witness_program.program().len() {
            WITNESS_V0_SCRIPTHASH_SIZE => {
                // BIP141 P2WSH: 32-byte witness v0 program (which encodes SHA256(script))
                if witness.is_empty() {
                    return Err(ScriptError::WitnessProgramWitnessEmpty);
                }

                let script_bytes = stack.last()?;
                let exec_script = Script::from_bytes(script_bytes);

                let exec_script_hash: [u8; 32] = exec_script.wscript_hash().to_byte_array();

                if exec_script_hash.as_slice() != witness_program.program().as_bytes() {
                    return Err(ScriptError::WitnessProgramMismatch);
                }

                execute_witness_script(
                    &stack,
                    &exec_script,
                    flags,
                    SigVersion::WitnessV0,
                    checker,
                    &mut ScriptExecutionData::default(),
                )?;
            }
            WITNESS_V0_KEYHASH_SIZE => {
                // BIP141 P2WPKH: 20-byte witness v0 program (which encodes Hash160(pubkey))
                if stack.len() != 2 {
                    return Err(ScriptError::WitnessProgramMismatch);
                }

                let exec_script = Builder::default()
                    .push_opcode(OP_DUP)
                    .push_opcode(OP_HASH160)
                    .push_slice(witness_program.program())
                    .push_opcode(OP_EQUALVERIFY)
                    .push_opcode(OP_CHECKSIG)
                    .into_script();

                execute_witness_script(
                    &stack,
                    &exec_script,
                    flags,
                    SigVersion::WitnessV0,
                    checker,
                    &mut ScriptExecutionData::default(),
                )?;
            }
            _ => return Err(ScriptError::WitnessProgramWrongLength),
        }
    } else if witness_version == WitnessVersion::V1
        && witness_program.program().len() == WITNESS_V0_SCRIPTHASH_SIZE
        && !is_p2sh
    {
        // BIP 341 Taproot: 32-byte non-P2SH witness v1 program (which encodes a P2C-tweaked pubkey)
        if !flags.intersects(VerificationFlags::TAPROOT) {
            return Ok(());
        }

        if stack.is_empty() {
            return Err(ScriptError::WitnessProgramWitnessEmpty);
        }

        // Handle annex.
        todo!("Taproot")
    } else if flags.intersects(VerificationFlags::DISCOURAGE_UPGRADABLE_WITNESS_PROGRAM) {
        return Err(ScriptError::DiscourageUpgradableWitnessProgram);
    }

    // Other version/size/p2sh combinations returns true for future softfork compatibility.
    // Ok(true)?
    Ok(())
}

fn execute_witness_script(
    stack_span: &[Vec<u8>],
    exec_script: &Script,
    flags: &VerificationFlags,
    sig_version: SigVersion,
    checker: &mut impl SignatureChecker,
    exec_data: &mut ScriptExecutionData,
) -> Result<(), ScriptError> {
    let mut stack = Stack::new(stack_span.to_vec(), true);

    if sig_version == SigVersion::Tapscript {
        // OP_SUCCESSx processing overrides everything, including stack element size limits
        for instruction in exec_script.instructions() {
            match instruction.map_err(ScriptError::ReadInstruction)? {
                Instruction::Op(opcode) => {
                    // New opcodes will be listed here. May use a different sigversion to modify existing opcodes.
                    if is_op_success(opcode.to_u8()) {
                        if flags.intersects(VerificationFlags::DISCOURAGE_OP_SUCCESS) {
                            return Err(ScriptError::DiscourageOpSuccess);
                        }
                        return Ok(());
                    }
                }
                Instruction::PushBytes(_) => return Err(ScriptError::BadOpcode),
            }
        }

        // Tapscript enforces initial stack size limits (altstack is empty here)
        if stack.len() > MAX_STACK_SIZE {
            return Err(ScriptError::StackSize);
        }
    }

    // Disallow stack item size > MAX_SCRIPT_ELEMENT_SIZE in witness stack
    if stack
        .iter()
        .any(|elem| elem.len() > MAX_SCRIPT_ELEMENT_SIZE)
    {
        return Err(ScriptError::PushSize);
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
        return Err(ScriptError::CleanStack);
    }

    if !stack.peek_bool()? {
        return Err(ScriptError::EvalFalse);
    }

    Ok(())
}

fn is_op_success(opcode: u8) -> bool {
    (opcode == 0x50) || (opcode >= 0x7b && opcode <= 0xb9)
}
