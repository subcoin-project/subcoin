mod multisig;
mod sig;

use crate::constants::{
    MAX_OPS_PER_SCRIPT, MAX_SCRIPT_ELEMENT_SIZE, MAX_STACK_SIZE, SEQUENCE_LOCKTIME_DISABLE_FLAG,
};
use crate::error::Error;
use crate::num::ScriptNum;
use crate::signature_checker::SignatureChecker;
use crate::stack::{Stack, StackError};
use crate::{ScriptExecutionData, SigVersion, VerifyFlags};
use bitcoin::hashes::{hash160, ripemd160, sha1, sha256, sha256d, Hash};
use bitcoin::script::Instruction;
use bitcoin::Script;
use std::ops::{Add, Neg, Sub};

pub use self::multisig::CheckMultiSigError;
pub use self::sig::CheckSigError;

pub fn eval_script<SC: SignatureChecker>(
    stack: &mut Stack,
    script: &Script,
    flags: &VerifyFlags,
    checker: &mut SC,
    sig_version: SigVersion,
    exec_data: &mut ScriptExecutionData,
) -> Result<bool, Error> {
    use crate::opcode::Opcode::*;

    if matches!(sig_version, SigVersion::Taproot) {
        return Err(Error::NoScriptExecution);
    }

    let mut alt_stack = Stack::with_flags(flags);

    // Create a vector of conditional execution states
    let mut exec_stack: Vec<bool> = Vec::new();

    let mut begincode = 0;
    let mut op_count = 0;
    let mut opcode_pos = 0;

    let step = |instruction: &Instruction| match instruction {
        Instruction::PushBytes(b) => b.as_bytes().len() + 1,
        Instruction::Op(_) => 1,
    };

    let instructions = if flags.intersects(VerifyFlags::MINIMALDATA) {
        script.instruction_indices_minimal()
    } else {
        script.instruction_indices()
    };

    let mut pc = 0;

    for instruction in instructions {
        let (_pos, instruction) = instruction.map_err(Error::ReadInstruction)?;

        pc += step(&instruction);

        let executing = exec_stack.iter().all(|x| *x);

        match instruction {
            Instruction::PushBytes(p) => {
                if p.len() > MAX_SCRIPT_ELEMENT_SIZE {
                    return Err(Error::PushSize);
                }
                // TODO: avoid allocation?
                stack.push(p.as_bytes().to_vec());
            }
            Instruction::Op(op) => {
                if matches!(sig_version, SigVersion::Base | SigVersion::WitnessV0) {
                    // Note how OP_RESERVED does not count towards the opcode limit.
                    if op.to_u8() > bitcoin::opcodes::all::OP_PUSHNUM_16.to_u8() {
                        op_count += 1;

                        if op_count > MAX_OPS_PER_SCRIPT {
                            return Err(Error::OpCount);
                        }
                    }
                }

                let opcode =
                    crate::opcode::Opcode::from_u8(op.to_u8()).ok_or(Error::UnknownOpcode(op))?;

                match opcode {
                    OP_0 | OP_PUSHBYTES_1 | OP_PUSHBYTES_2 | OP_PUSHBYTES_3 | OP_PUSHBYTES_4
                    | OP_PUSHBYTES_5 | OP_PUSHBYTES_6 | OP_PUSHBYTES_7 | OP_PUSHBYTES_8
                    | OP_PUSHBYTES_9 | OP_PUSHBYTES_10 | OP_PUSHBYTES_11 | OP_PUSHBYTES_12
                    | OP_PUSHBYTES_13 | OP_PUSHBYTES_14 | OP_PUSHBYTES_15 | OP_PUSHBYTES_16
                    | OP_PUSHBYTES_17 | OP_PUSHBYTES_18 | OP_PUSHBYTES_19 | OP_PUSHBYTES_20
                    | OP_PUSHBYTES_21 | OP_PUSHBYTES_22 | OP_PUSHBYTES_23 | OP_PUSHBYTES_24
                    | OP_PUSHBYTES_25 | OP_PUSHBYTES_26 | OP_PUSHBYTES_27 | OP_PUSHBYTES_28
                    | OP_PUSHBYTES_29 | OP_PUSHBYTES_30 | OP_PUSHBYTES_31 | OP_PUSHBYTES_32
                    | OP_PUSHBYTES_33 | OP_PUSHBYTES_34 | OP_PUSHBYTES_35 | OP_PUSHBYTES_36
                    | OP_PUSHBYTES_37 | OP_PUSHBYTES_38 | OP_PUSHBYTES_39 | OP_PUSHBYTES_40
                    | OP_PUSHBYTES_41 | OP_PUSHBYTES_42 | OP_PUSHBYTES_43 | OP_PUSHBYTES_44
                    | OP_PUSHBYTES_45 | OP_PUSHBYTES_46 | OP_PUSHBYTES_47 | OP_PUSHBYTES_48
                    | OP_PUSHBYTES_49 | OP_PUSHBYTES_50 | OP_PUSHBYTES_51 | OP_PUSHBYTES_52
                    | OP_PUSHBYTES_53 | OP_PUSHBYTES_54 | OP_PUSHBYTES_55 | OP_PUSHBYTES_56
                    | OP_PUSHBYTES_57 | OP_PUSHBYTES_58 | OP_PUSHBYTES_59 | OP_PUSHBYTES_60
                    | OP_PUSHBYTES_61 | OP_PUSHBYTES_62 | OP_PUSHBYTES_63 | OP_PUSHBYTES_64
                    | OP_PUSHBYTES_65 | OP_PUSHBYTES_66 | OP_PUSHBYTES_67 | OP_PUSHBYTES_68
                    | OP_PUSHBYTES_69 | OP_PUSHBYTES_70 | OP_PUSHBYTES_71 | OP_PUSHBYTES_72
                    | OP_PUSHBYTES_73 | OP_PUSHBYTES_74 | OP_PUSHBYTES_75 | OP_PUSHDATA1
                    | OP_PUSHDATA2 | OP_PUSHDATA4 => {
                        unreachable!("Instruction::Op(opcode) contains non-push opcode only");
                    }

                    // Constants
                    OP_1NEGATE | OP_1 | OP_2 | OP_3 | OP_4 | OP_5 | OP_6 | OP_7 | OP_8 | OP_9
                    | OP_10 | OP_11 | OP_12 | OP_13 | OP_14 | OP_15 | OP_16 => {
                        let value = (opcode as u8 as i32).wrapping_sub(OP_1 as u8 as i32 - 1);
                        stack.push_num(value as i64);
                    }

                    // Flow control
                    OP_NOP => {}
                    OP_IF | OP_NOTIF => {
                        let mut value = false;

                        if executing {
                            let top = stack.pop()?;

                            // Tapscript requires minimal IF/NOTIF inputs as a consensus rule.
                            if matches!(sig_version, SigVersion::Tapscript) {
                                // The input argument to the OP_IF and OP_NOTIF opcodes must be either
                                // exactly 0 (the empty vector) or exactly 1 (the one-byte vector with value 1).
                                if top.len() > 1 || (top.len() == 1 && top[0] != 1) {
                                    return Err(Error::TaprootMinimalif);
                                }
                            }

                            // Under witness v0 rules it is only a policy rule, enabled through SCRIPT_VERIFY_MINIMALIF.
                            if matches!(sig_version, SigVersion::WitnessV0)
                                && flags.intersects(VerifyFlags::MINIMALIF)
                            {
                                #[allow(clippy::collapsible_if)]
                                if top.len() > 1 || (top.len() == 1 && top[0] != 1) {
                                    return Err(Error::Minimalif);
                                }
                            }

                            value = crate::stack::cast_to_bool(&top);

                            if opcode == OP_NOTIF {
                                value = !value;
                            }
                        }

                        exec_stack.push(value);
                    }
                    OP_ELSE => {
                        // Toggle top.
                        if let Some(last) = exec_stack.last_mut() {
                            *last = !*last;
                        } else {
                            return Err(Error::UnbalancedConditional);
                        }
                    }
                    OP_ENDIF => {
                        if exec_stack.is_empty() {
                            return Err(Error::UnbalancedConditional);
                        }
                        exec_stack.pop();
                    }
                    OP_VERIFY => {
                        let exec_value = stack.pop_bool()?;
                        if !exec_value {
                            return Err(Error::Verify(op));
                        }
                    }
                    OP_RETURN => return Err(Error::OpReturn),

                    // Stack
                    OP_TOALTSTACK => {
                        alt_stack.push(stack.pop()?);
                    }
                    OP_FROMALTSTACK => {
                        let v = alt_stack
                            .pop()
                            .map_err(|_| Error::InvalidAltStackOperation)?;
                        stack.push(v);
                    }
                    OP_2DROP => stack.drop(2)?,
                    OP_2DUP => stack.dup(2)?,
                    OP_3DUP => stack.dup(3)?,
                    OP_2OVER => stack.over(2)?,
                    OP_2ROT => stack.rot(2)?,
                    OP_2SWAP => stack.swap(2)?,
                    OP_IFDUP => {
                        if stack.peek_bool()? {
                            stack.dup(1)?;
                        }
                    }
                    OP_DEPTH => {
                        // Push the current number of stack items onto the stack.
                        stack.push_num(stack.len() as i64);
                    }
                    OP_DROP => stack.drop(1)?,
                    OP_DUP => stack.dup(1)?,
                    OP_NIP => stack.nip()?,
                    OP_OVER => stack.over(1)?,
                    OP_PICK | OP_ROLL => {
                        // Pop the top stack element as N.
                        let n = stack.pop_num()?.value();
                        if n < 0 || n >= stack.len() as i64 {
                            return Err(StackError::InvalidOperation.into());
                        }
                        let v = if opcode == OP_PICK {
                            // Copy the Nth stack element to the top.
                            stack.top(n as usize)?.clone()
                        } else {
                            // Move the Nth stack element to the top.
                            stack.remove(n as usize)?
                        };
                        stack.push(v);
                    }
                    OP_ROT => stack.rot(1)?,
                    OP_SWAP => stack.swap(1)?,
                    OP_TUCK => stack.tuck()?,

                    // Splice
                    OP_CAT | OP_SUBSTR | OP_LEFT | OP_RIGHT => {
                        return Err(Error::DisabledOpcode(op))
                    }
                    OP_SIZE => {
                        stack.push_num(stack.last()?.len() as i64);
                    }

                    // Bitwise logic
                    OP_INVERT | OP_AND | OP_OR | OP_XOR => return Err(Error::DisabledOpcode(op)),
                    OP_EQUAL => {
                        let equal = stack.pop()? == stack.pop()?;
                        stack.push_bool(equal);
                    }
                    OP_EQUALVERIFY => {
                        let equal = stack.pop()? == stack.pop()?;
                        if !equal {
                            return Err(Error::Verify(op));
                        }
                    }

                    // Arithmetic
                    OP_2MUL | OP_2DIV | OP_MUL | OP_DIV | OP_MOD | OP_LSHIFT | OP_RSHIFT => {
                        return Err(Error::DisabledOpcode(op));
                    }
                    OP_1ADD => {
                        let n = stack.pop_num()?.add(1.into())?;
                        stack.push_num(n);
                    }
                    OP_1SUB => {
                        let n = stack.pop_num()?.sub(1.into())?;
                        stack.push_num(n);
                    }
                    OP_NEGATE => {
                        let n = stack.pop_num()?.neg()?;
                        stack.push_num(n);
                    }
                    OP_ABS => {
                        let n = stack.pop_num()?.abs();
                        stack.push_num(n);
                    }
                    OP_NOT => {
                        let n = ScriptNum::from(stack.pop_num()?.is_zero());
                        stack.push_num(n);
                    }
                    OP_0NOTEQUAL => {
                        let n = ScriptNum::from(!stack.pop_num()?.is_zero());
                        stack.push_num(n);
                    }
                    OP_ADD => {
                        let v1 = stack.pop_num()?;
                        let v2 = stack.pop_num()?;
                        stack.push_num((v1 + v2)?);
                    }
                    OP_SUB => {
                        let v1 = stack.pop_num()?;
                        let v2 = stack.pop_num()?;
                        stack.push_num((v2 - v1)?);
                    }
                    OP_BOOLAND => {
                        let v1 = !stack.pop_num()?.is_zero();
                        let v2 = !stack.pop_num()?.is_zero();
                        stack.push_num(v1 && v2);
                    }
                    OP_BOOLOR => {
                        let v1 = !stack.pop_num()?.is_zero();
                        let v2 = !stack.pop_num()?.is_zero();
                        stack.push_num(v1 || v2);
                    }
                    OP_NUMEQUAL => {
                        let v1 = stack.pop_num()?;
                        let v2 = stack.pop_num()?;
                        stack.push_num(v1 == v2);
                    }
                    OP_NUMEQUALVERIFY => {
                        let v1 = stack.pop_num()?;
                        let v2 = stack.pop_num()?;
                        if v1 != v2 {
                            return Err(Error::Verify(op));
                        }
                    }
                    OP_NUMNOTEQUAL => {
                        let v1 = stack.pop_num()?;
                        let v2 = stack.pop_num()?;
                        stack.push_num(v1 != v2);
                    }
                    OP_LESSTHAN => {
                        let v1 = stack.pop_num()?;
                        let v2 = stack.pop_num()?;
                        stack.push_num(v2 < v1);
                    }
                    OP_GREATERTHAN => {
                        let v1 = stack.pop_num()?;
                        let v2 = stack.pop_num()?;
                        stack.push_num(v2 > v1);
                    }
                    OP_LESSTHANOREQUAL => {
                        let v1 = stack.pop_num()?;
                        let v2 = stack.pop_num()?;
                        stack.push_num(v2 <= v1);
                    }
                    OP_GREATERTHANOREQUAL => {
                        let v1 = stack.pop_num()?;
                        let v2 = stack.pop_num()?;
                        stack.push_num(v2 >= v1);
                    }
                    OP_MIN => {
                        let v1 = stack.pop_num()?;
                        let v2 = stack.pop_num()?;
                        stack.push_num(v1.min(v2));
                    }
                    OP_MAX => {
                        let v1 = stack.pop_num()?;
                        let v2 = stack.pop_num()?;
                        stack.push_num(v1.max(v2));
                    }
                    OP_WITHIN => {
                        // [v3 v2 v1] = [x min max]
                        let v1 = stack.pop_num()?;
                        let v2 = stack.pop_num()?;
                        let v3 = stack.pop_num()?;
                        stack.push_bool((v2..v1).contains(&v3));
                    }

                    // Crypto
                    OP_RIPEMD160 => {
                        let v = ripemd160::Hash::hash(&stack.pop()?);
                        stack.push(v.to_byte_array().to_vec());
                    }
                    OP_SHA1 => {
                        let v = sha1::Hash::hash(&stack.pop()?);
                        stack.push(v.to_byte_array().to_vec());
                    }
                    OP_SHA256 => {
                        let v = sha256::Hash::hash(&stack.pop()?);
                        stack.push(v.to_byte_array().to_vec());
                    }
                    OP_HASH160 => {
                        let v = hash160::Hash::hash(&stack.pop()?);
                        stack.push(v.to_byte_array().to_vec());
                    }
                    OP_HASH256 => {
                        let v = sha256d::Hash::hash(&stack.pop()?);
                        stack.push(v.to_byte_array().to_vec());
                    }
                    OP_CODESEPARATOR => {
                        // Use of OP_CODESEPARATOR is rejected in pre-segwit script.
                        if matches!(sig_version, SigVersion::Base)
                            && flags.intersects(VerifyFlags::CONST_SCRIPTCODE)
                        {
                            return Err(Error::OpCodeSeparator);
                        }

                        begincode = pc;
                        exec_data.codeseparator_pos = opcode_pos;
                    }
                    OP_CHECKSIG | OP_CHECKSIGVERIFY => {
                        // [sig pubkey] -> bool
                        let pubkey = stack.pop()?;
                        let sig = stack.pop()?;

                        let success = sig::handle_checksig(
                            &sig,
                            &pubkey,
                            script,
                            begincode,
                            exec_data,
                            flags,
                            checker,
                            sig_version,
                        )?;

                        match opcode {
                            OP_CHECKSIG => {
                                stack.push_bool(success);
                            }
                            OP_CHECKSIGVERIFY if !success => {
                                return Err(Error::Verify(op));
                            }
                            _ => {}
                        }
                    }
                    OP_CHECKMULTISIG | OP_CHECKMULTISIGVERIFY => {
                        let multisig_op = if opcode == OP_CHECKMULTISIG {
                            multisig::MultiSigOp::CheckMultiSig
                        } else {
                            multisig::MultiSigOp::CheckMultiSigVerify
                        };

                        multisig::handle_checkmultisig(
                            stack,
                            flags,
                            begincode,
                            script,
                            sig_version,
                            checker,
                            multisig_op,
                            &mut op_count,
                        )?;
                    }
                    OP_CHECKSIGADD => {
                        // (sig num pubkey -- num)
                        let pubkey = stack.pop()?;
                        let num = stack.pop_num()?;
                        let sig = stack.pop()?;

                        let success = sig::handle_checksig(
                            &sig,
                            &pubkey,
                            script,
                            begincode,
                            exec_data,
                            flags,
                            checker,
                            sig_version,
                        )
                        .map_err(Error::CheckSig)?;

                        let v = num.value() + if success { 1 } else { 0 };
                        stack.push_num(v);
                    }

                    // Locktime, CLTV
                    OP_CHECKLOCKTIMEVERIFY => {
                        // not enabled; treat as a NOP3
                        if !flags.intersects(VerifyFlags::CHECKLOCKTIMEVERIFY) {
                            continue;
                        }

                        // Note that elsewhere numeric opcodes are limited to
                        // operands in the range -2**31+1 to 2**31-1, however it is
                        // legal for opcodes to produce results exceeding that
                        // range. This limitation is implemented by CScriptNum's
                        // default 4-byte limit.
                        //
                        // If we kept to that limit we'd have a year 2038 problem,
                        // even though the nLockTime field in transactions
                        // themselves is uint32 which only becomes meaningless
                        // after the year 2106.
                        //
                        // Thus as a special case we tell CScriptNum to accept up
                        // to 5-byte bignums, which are good until 2**39-1, well
                        // beyond the 2**32-1 limit of the nLockTime field itself.
                        let lock_time = stack.pop_num_with_max_size(5)?;

                        // In the rare event that the argument may be < 0 due to
                        // some arithmetic being done first, you can always use
                        // 0 MAX CHECKLOCKTIMEVERIFY.
                        if lock_time.is_negative() {
                            return Err(Error::NegativeLocktime);
                        }

                        if !checker.check_lock_time(lock_time) {
                            return Err(Error::UnsatisfiedLocktime);
                        }
                    }
                    // CSV
                    OP_CHECKSEQUENCEVERIFY => {
                        // not enabled; treat as a NOP3
                        if !flags.intersects(VerifyFlags::CHECKSEQUENCEVERIFY) {
                            continue;
                        }

                        // opcodeCheckSequenceVerify compares the top item on the data stack to the
                        // LockTime field of the transaction containing the script signature
                        // validating if the transaction outputs are spendable yet.  If flag
                        // ScriptVerifyCheckSequenceVerify is not set, the code continues as if OP_NOP3
                        // were executed.
                        let sequence = stack.pop_num_with_max_size(5)?;

                        if sequence.is_negative() {
                            return Err(Error::NegativeLocktime);
                        }

                        if (sequence.value() & SEQUENCE_LOCKTIME_DISABLE_FLAG as i64) == 0
                            && !checker.check_sequence(sequence)
                        {
                            return Err(Error::UnsatisfiedLocktime);
                        }
                    }

                    // Reserved words
                    OP_RESERVED | OP_VER | OP_RESERVED1 | OP_RESERVED2 => {
                        if executing {
                            return Err(Error::DisabledOpcode(op));
                        }
                    }
                    OP_VERIF | OP_VERNOTIF => {
                        return Err(Error::DisabledOpcode(op));
                    }
                    OP_NOP1 | OP_NOP4 | OP_NOP5 | OP_NOP6 | OP_NOP7 | OP_NOP8 | OP_NOP9
                    | OP_NOP10 => {
                        if flags.intersects(VerifyFlags::DISCOURAGE_UPGRADABLE_NOPS) {
                            return Err(Error::DiscourageUpgradableNops);
                        }
                    }
                }
            }
        }

        opcode_pos += 1;

        if stack.len() + alt_stack.len() > MAX_STACK_SIZE {
            return Err(Error::StackSize);
        }
    }

    if !exec_stack.is_empty() {
        return Err(Error::UnbalancedConditional);
    }

    let success = !stack.is_empty() && stack.peek_bool()?;

    Ok(success)
}
