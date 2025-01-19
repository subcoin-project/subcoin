mod multisig;
mod sig;

use crate::num::{NumError, ScriptNum};
use crate::signature_checker::SignatureChecker;
use crate::stack::{Stack, StackError};
use crate::{ScriptExecutionData, SigVersion, VerificationFlags};
use bitcoin::hashes::{hash160, ripemd160, sha1, sha256, sha256d, Hash};
use bitcoin::script::Instruction;
use bitcoin::Script;
use std::ops::{Add, Neg, Sub};

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum Error {
    #[error("invalid stack operation")]
    InvalidStackOperation,
    #[error("{0} is disabled")]
    DisabledOpcode(bitcoin::opcodes::Opcode),
    #[error("{0} is unknown")]
    UnknownOpcode(bitcoin::opcodes::Opcode),
    #[error("negative locktime")]
    NegativeLocktime,
    #[error("unsatisfied locktime")]
    UnsatisfiedLocktime,
    #[error("unbalanced conditional")]
    UnbalancedConditional,
    #[error("disable upgrable nops")]
    DiscourageUpgradableNops,
    #[error("failed verify operation: {0:?}")]
    FailedVerify(bitcoin::opcodes::Opcode),
    #[error("return opcode")]
    ReturnOpcode,
    #[error("invalid alt stack operation")]
    InvalidAltStackOperation,
    #[error("rust-bitcoin script error: {0:?}")]
    Script(bitcoin::script::Error),
    #[error(transparent)]
    Stack(#[from] StackError),
    #[error(transparent)]
    Num(#[from] NumError),
    #[error(transparent)]
    Sig(#[from] sig::SigError),
    #[error(transparent)]
    Multisig(#[from] multisig::MultisigError),
}

type Result<T> = std::result::Result<T, Error>;

pub fn eval_script(
    stack: &mut Stack,
    script: &Script,
    flags: VerificationFlags,
    checker: &dyn SignatureChecker,
    sig_version: SigVersion,
    exec_data: &mut ScriptExecutionData,
) -> Result<bool> {
    use crate::opcode::Opcode::*;

    let mut alt_stack = Stack::default();

    // Create a vector of conditional execution states
    let mut exec_stack: Vec<bool> = Vec::new();

    let mut begincode = 0;

    for instruction in script.instruction_indices() {
        let (pc, instruction) = instruction.map_err(Error::Script)?;

        match instruction {
            Instruction::PushBytes(p) => {
                stack.push(p.as_bytes().to_vec());
            }
            Instruction::Op(op) => {
                let opcode =
                    crate::opcode::Opcode::from_u8(op.to_u8()).ok_or(Error::UnknownOpcode(op))?;

                let executing = exec_stack.iter().all(|x| *x);

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
                        // TODO: tapscript support
                        let exec_value = pop_exec_value(stack, executing)?;
                        exec_stack.push(if opcode == OP_IF {
                            exec_value
                        } else {
                            !exec_value
                        });
                    }
                    OP_ELSE => {
                        // toggle top.
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
                            return Err(Error::FailedVerify(op));
                        }
                    }
                    OP_RETURN => return Err(Error::ReturnOpcode),

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
                            return Err(Error::InvalidStackOperation);
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
                            return Err(Error::FailedVerify(op));
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
                            return Err(Error::FailedVerify(op));
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
                        // [x min max]
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
                        begincode = pc;
                    }
                    OP_CHECKSIG | OP_CHECKSIGVERIFY => {
                        // [sig pubkey] -> bool
                        let pubkey = stack.pop()?;
                        let sig = stack.pop()?;

                        let success = sig::eval_checksig(
                            &sig,
                            &pubkey,
                            script,
                            begincode,
                            &exec_data,
                            &flags,
                            checker,
                            sig_version,
                        )?;

                        match opcode {
                            OP_CHECKSIG => {
                                stack.push_bool(success);
                            }
                            OP_CHECKSIGVERIFY if !success => {
                                return Err(Error::FailedVerify(op));
                            }
                            _ => {}
                        }
                    }
                    OP_CHECKMULTISIG | OP_CHECKMULTISIGVERIFY => {
                        multisig::execute_checkmultisig(
                            stack,
                            &flags,
                            begincode,
                            script,
                            sig_version,
                            checker,
                            if opcode == OP_CHECKMULTISIG {
                                multisig::Operation::CheckMultisig
                            } else {
                                multisig::Operation::CheckMultisigVerify
                            },
                        )?;
                    }
                    OP_CHECKSIGADD => {
                        todo!("checksigadd for tapscript")
                    }

                    // Locktime
                    OP_CHECKLOCKTIMEVERIFY => {
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
                    OP_CHECKSEQUENCEVERIFY => {
                        // Below flags apply in the context of BIP 68
                        // If this flag set, CTxIn::nSequence is NOT interpreted as a
                        // relative lock-time.
                        const SEQUENCE_LOCKTIME_DISABLE_FLAG: u32 = 1u32 << 31;

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
                        return Err(Error::DiscourageUpgradableNops);
                    }
                }
            }
        }
    }

    if !exec_stack.is_empty() {
        return Err(Error::UnbalancedConditional);
    }

    let success = !stack.is_empty() && stack.peek_bool()?;

    Ok(success)
}

fn pop_exec_value(stack: &mut Stack, executing: bool) -> Result<bool> {
    if executing {
        stack.pop_bool().map_err(|_| Error::UnbalancedConditional)
    } else {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signature_checker::SkipSignatureCheck;
    use bitcoin::hex::FromHex;
    use bitcoin::opcodes::all::*;
    use bitcoin::script::Builder;

    struct EvalResult {
        /// Result of [`eval_script`].
        result: Result<bool>,
        /// Stack of after the evaluation if no error occurs.
        expected_stack: Option<Stack>,
    }

    impl EvalResult {
        fn ok(success: bool, stack: Stack) -> Self {
            Self {
                result: Ok(success),
                expected_stack: Some(stack),
            }
        }

        fn err(err: impl Into<Error>) -> Self {
            Self {
                result: Err(err.into()),
                expected_stack: None,
            }
        }
    }

    fn basic_test(script: &Script, eval_result: EvalResult) {
        let EvalResult {
            result: expected,
            expected_stack,
        } = eval_result;

        let flags = VerificationFlags::P2SH;
        let version = SigVersion::Base;
        let checker = SkipSignatureCheck::new();
        let mut stack = Stack::default();
        let eval_script_result = eval_script(
            &mut stack,
            script,
            flags,
            &checker,
            version,
            &mut ScriptExecutionData::default(),
        );
        assert_eq!(eval_script_result, expected);
        if expected.is_ok() {
            assert_eq!(
                stack,
                expected_stack.expect("Stack must be checked if eval result is ok")
            );
        }
    }

    #[test]
    fn test_equal() {
        let script = Builder::new()
            .push_slice(&[0x4])
            .push_slice(&[0x4])
            .push_opcode(OP_EQUAL)
            .into_script();
        let result = EvalResult::ok(true, Stack::from(vec![vec![1]]));
        basic_test(&script, result);
    }

    #[test]
    fn test_equal_false() {
        let script = Builder::default()
            .push_slice(&[0x4])
            .push_slice(&[0x3])
            .push_opcode(OP_EQUAL)
            .into_script();
        let result = EvalResult::ok(false, Stack::from(vec![vec![]]));
        basic_test(&script, result);
    }

    #[test]
    fn test_equal_invalid_stack() {
        let script = Builder::default()
            .push_slice(&[0x4])
            .push_opcode(OP_EQUAL)
            .into_script();
        let result = EvalResult::err(StackError::InvalidOperation);
        basic_test(&script, result);
    }

    #[test]
    fn test_equal_verify() {
        let script = Builder::default()
            .push_slice(&[0x4])
            .push_slice(&[0x4])
            .push_opcode(OP_EQUALVERIFY)
            .into_script();
        let result = EvalResult::ok(false, Stack::empty());
        basic_test(&script, result);
    }

    #[test]
    fn test_equal_verify_failed() {
        let script = Builder::default()
            .push_slice(&[0x4])
            .push_slice(&[0x3])
            .push_opcode(OP_EQUALVERIFY)
            .into_script();
        let result = EvalResult::err(Error::FailedVerify(OP_EQUALVERIFY));
        basic_test(&script, result);
    }

    #[test]
    fn test_size() {
        let script = Builder::default()
            .push_slice(&[0x12, 0x34])
            .push_opcode(OP_SIZE)
            .into_script();
        let mut stack = Stack::default();
        stack.push(vec![0x12, 0x34]).push(vec![0x2]);
        let result = EvalResult::ok(true, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_size_false() {
        let script = Builder::default()
            .push_slice(&[])
            .push_opcode(OP_SIZE)
            .into_script();
        let mut stack = Stack::default();
        stack.push(vec![]).push_num(0);
        let result = EvalResult::ok(false, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_size_invalid_stack() {
        let script = Builder::default().push_opcode(OP_SIZE).into_script();
        let result = EvalResult::err(StackError::InvalidOperation);
        basic_test(&script, result);
    }

    #[test]
    fn test_hash256() {
        let script = Builder::default()
            .push_slice(b"hello")
            .push_opcode(OP_HASH256)
            .into_script();
        let stack =
            vec![
                Vec::from_hex("9595c9df90075148eb06860365df33584b75bff782a510c6cd4883a419833d50")
                    .unwrap(),
            ]
            .into();
        let result = EvalResult::ok(true, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_hash256_invalid_stack() {
        let script = Builder::default().push_opcode(OP_HASH256).into_script();
        let result = EvalResult::err(StackError::InvalidOperation);
        basic_test(&script, result);
    }

    #[test]
    fn test_ripemd160() {
        let script = Builder::default()
            .push_slice(b"hello")
            .push_opcode(OP_RIPEMD160)
            .into_script();
        let stack = vec![Vec::from_hex("108f07b8382412612c048d07d13f814118445acd").unwrap()].into();
        let result = EvalResult::ok(true, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_ripemd160_invalid_stack() {
        let script = Builder::default().push_opcode(OP_RIPEMD160).into_script();
        let result = EvalResult::err(StackError::InvalidOperation);
        basic_test(&script, result);
    }

    #[test]
    fn test_sha1() {
        let script = Builder::default()
            .push_slice(b"hello")
            .push_opcode(OP_SHA1)
            .into_script();
        let stack = vec![Vec::from_hex("aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d").unwrap()].into();
        let result = EvalResult::ok(true, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_sha1_invalid_stack() {
        let script = Builder::default().push_opcode(OP_SHA1).into_script();
        let result = EvalResult::err(StackError::InvalidOperation);
        basic_test(&script, result);
    }

    #[test]
    fn test_sha256() {
        let script = Builder::default()
            .push_slice(b"hello")
            .push_opcode(OP_SHA256)
            .into_script();
        let stack =
            vec![
                Vec::from_hex("2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824")
                    .unwrap(),
            ]
            .into();
        let result = EvalResult::ok(true, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_sha256_invalid_stack() {
        let script = Builder::default().push_opcode(OP_SHA256).into_script();
        let result = EvalResult::err(StackError::InvalidOperation);
        basic_test(&script, result);
    }

    #[test]
    fn test_1add() {
        let script = Builder::default()
            .push_int(5)
            .push_opcode(OP_1ADD)
            .into_script();
        let stack = vec![ScriptNum::from(6).to_bytes()].into();
        let result = EvalResult::ok(true, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_1add_invalid_stack() {
        let script = Builder::default().push_opcode(OP_1ADD).into_script();
        let result = EvalResult::err(StackError::InvalidOperation);
        basic_test(&script, result);
    }

    #[test]
    fn test_1sub() {
        let script = Builder::default()
            .push_int(5)
            .push_opcode(OP_1SUB)
            .into_script();
        let stack = vec![ScriptNum::from(4).to_bytes()].into();
        let result = EvalResult::ok(true, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_1sub_invalid_stack() {
        let script = Builder::default().push_opcode(OP_1SUB).into_script();
        let result = EvalResult::err(StackError::InvalidOperation);
        basic_test(&script, result);
    }

    #[test]
    fn test_negate() {
        let script = Builder::default()
            .push_int(5)
            .push_opcode(OP_NEGATE)
            .into_script();
        let stack = vec![ScriptNum::from(-5).to_bytes()].into();
        let result = EvalResult::ok(true, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_negate_negative() {
        let script = Builder::default()
            .push_int(-5)
            .push_opcode(OP_NEGATE)
            .into_script();
        let stack = vec![ScriptNum::from(5).to_bytes()].into();
        let result = EvalResult::ok(true, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_negate_invalid_stack() {
        let script = Builder::default().push_opcode(OP_NEGATE).into_script();
        let result = EvalResult::err(StackError::InvalidOperation);
        basic_test(&script, result);
    }

    #[test]
    fn test_abs() {
        let script = Builder::default()
            .push_int(5)
            .push_opcode(OP_ABS)
            .into_script();
        let stack = vec![ScriptNum::from(5).to_bytes()].into();
        let result = EvalResult::ok(true, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_abs_negative() {
        let script = Builder::default()
            .push_int(-5)
            .push_opcode(OP_ABS)
            .into_script();
        let stack = vec![ScriptNum::from(5).to_bytes()].into();
        let result = EvalResult::ok(true, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_abs_invalid_stack() {
        let script = Builder::default().push_opcode(OP_ABS).into_script();
        let result = EvalResult::err(StackError::InvalidOperation);
        basic_test(&script, result);
    }

    #[test]
    fn test_not() {
        let script = Builder::default()
            .push_int(4)
            .push_opcode(OP_NOT)
            .into_script();
        let stack = vec![ScriptNum::from(0).to_bytes()].into();
        let result = EvalResult::ok(false, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_not_zero() {
        let script = Builder::default()
            .push_int(0.into())
            .push_opcode(OP_NOT)
            .into_script();
        let stack = vec![ScriptNum::from(1).to_bytes()].into();
        let result = EvalResult::ok(true, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_not_invalid_stack() {
        let script = Builder::default().push_opcode(OP_NOT).into_script();
        let result = EvalResult::err(StackError::InvalidOperation);
        basic_test(&script, result);
    }

    #[test]
    fn test_0notequal() {
        let script = Builder::default()
            .push_int(4)
            .push_opcode(OP_0NOTEQUAL)
            .into_script();
        let stack = vec![ScriptNum::from(1).to_bytes()].into();
        let result = EvalResult::ok(true, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_0notequal_zero() {
        let script = Builder::default()
            .push_int(0)
            .push_opcode(OP_0NOTEQUAL)
            .into_script();
        let stack = vec![ScriptNum::from(0).to_bytes()].into();
        let result = EvalResult::ok(false, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_0notequal_invalid_stack() {
        let script = Builder::default().push_opcode(OP_0NOTEQUAL).into_script();
        let result = EvalResult::err(StackError::InvalidOperation);
        basic_test(&script, result);
    }

    #[test]
    fn test_add() {
        let script = Builder::default()
            .push_int(2)
            .push_int(3)
            .push_opcode(OP_ADD)
            .into_script();
        let stack = vec![ScriptNum::from(5).to_bytes()].into();
        let result = EvalResult::ok(true, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_add_invalid_stack() {
        let script = Builder::default()
            .push_int(2)
            .push_opcode(OP_ADD)
            .into_script();
        let result = EvalResult::err(StackError::InvalidOperation);
        basic_test(&script, result);
    }

    #[test]
    fn test_sub() {
        let script = Builder::default()
            .push_int(3)
            .push_int(2)
            .push_opcode(OP_SUB)
            .into_script();
        let stack = vec![ScriptNum::from(1).to_bytes()].into();
        let result = EvalResult::ok(true, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_sub_invalid_stack() {
        let script = Builder::default()
            .push_int(2)
            .push_opcode(OP_SUB)
            .into_script();
        let result = EvalResult::err(StackError::InvalidOperation);
        basic_test(&script, result);
    }

    #[test]
    fn test_booland() {
        let script = Builder::default()
            .push_int(3)
            .push_int(2)
            .push_opcode(OP_BOOLAND)
            .into_script();
        let stack = vec![ScriptNum::from(1).to_bytes()].into();
        let result = EvalResult::ok(true, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_booland_first() {
        let script = Builder::default()
            .push_int(2)
            .push_int(0)
            .push_opcode(OP_BOOLAND)
            .into_script();
        let stack = vec![ScriptNum::from(0).to_bytes()].into();
        let result = EvalResult::ok(false, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_booland_second() {
        let script = Builder::default()
            .push_int(0)
            .push_int(3)
            .push_opcode(OP_BOOLAND)
            .into_script();
        let stack = vec![ScriptNum::from(0).to_bytes()].into();
        let result = EvalResult::ok(false, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_booland_none() {
        let script = Builder::default()
            .push_int(0)
            .push_int(0)
            .push_opcode(OP_BOOLAND)
            .into_script();
        let stack = vec![ScriptNum::from(0).to_bytes()].into();
        let result = EvalResult::ok(false, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_booland_invalid_stack() {
        let script = Builder::default()
            .push_int(0)
            .push_opcode(OP_BOOLAND)
            .into_script();
        let result = EvalResult::err(StackError::InvalidOperation);
        basic_test(&script, result);
    }

    #[test]
    fn test_boolor() {
        let script = Builder::default()
            .push_int(3)
            .push_int(2)
            .push_opcode(OP_BOOLOR)
            .into_script();
        let stack = vec![ScriptNum::from(1).to_bytes()].into();
        let result = EvalResult::ok(true, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_boolor_first() {
        let script = Builder::default()
            .push_int(2)
            .push_int(0)
            .push_opcode(OP_BOOLOR)
            .into_script();
        let stack = vec![ScriptNum::from(1).to_bytes()].into();
        let result = EvalResult::ok(true, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_boolor_second() {
        let script = Builder::default()
            .push_int(0)
            .push_int(3)
            .push_opcode(OP_BOOLOR)
            .into_script();
        let stack = vec![ScriptNum::from(1).to_bytes()].into();
        let result = EvalResult::ok(true, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_boolor_none() {
        let script = Builder::default()
            .push_int(0)
            .push_int(0)
            .push_opcode(OP_BOOLOR)
            .into_script();
        let stack = vec![ScriptNum::from(0).to_bytes()].into();
        let result = EvalResult::ok(false, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_boolor_invalid_stack() {
        let script = Builder::default()
            .push_int(0)
            .push_opcode(OP_BOOLOR)
            .into_script();
        let result = EvalResult::err(StackError::InvalidOperation);
        basic_test(&script, result);
    }

    #[test]
    fn test_numequal() {
        let script = Builder::default()
            .push_int(2.into())
            .push_int(2.into())
            .push_opcode(OP_NUMEQUAL)
            .into_script();
        let stack = vec![ScriptNum::from(1).to_bytes()].into();
        let result = EvalResult::ok(true, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_numequal_not() {
        let script = Builder::default()
            .push_int(2)
            .push_int(3)
            .push_opcode(OP_NUMEQUAL)
            .into_script();
        let stack = vec![ScriptNum::from(0).to_bytes()].into();
        let result = EvalResult::ok(false, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_numequal_invalid_stack() {
        let script = Builder::default()
            .push_int(2)
            .push_opcode(OP_NUMEQUAL)
            .into_script();
        let result = EvalResult::err(StackError::InvalidOperation);
        basic_test(&script, result);
    }

    #[test]
    fn test_numequalverify() {
        let script = Builder::default()
            .push_int(2)
            .push_int(2)
            .push_opcode(OP_NUMEQUALVERIFY)
            .into_script();
        let result = EvalResult::ok(false, Stack::empty());
        basic_test(&script, result);
    }

    #[test]
    fn test_numequalverify_failed() {
        let script = Builder::default()
            .push_int(2)
            .push_int(3)
            .push_opcode(OP_NUMEQUALVERIFY)
            .into_script();
        let result = EvalResult::err(Error::FailedVerify(OP_NUMEQUALVERIFY));
        basic_test(&script, result);
    }

    #[test]
    fn test_numequalverify_invalid_stack() {
        let script = Builder::default()
            .push_int(2)
            .push_opcode(OP_NUMEQUALVERIFY)
            .into_script();
        let result = EvalResult::err(StackError::InvalidOperation);
        basic_test(&script, result);
    }

    #[test]
    fn test_numnotequal() {
        let script = Builder::default()
            .push_int(2)
            .push_int(3)
            .push_opcode(OP_NUMNOTEQUAL)
            .into_script();
        let stack = vec![ScriptNum::from(1).to_bytes()].into();
        let result = EvalResult::ok(true, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_numnotequal_not() {
        let script = Builder::default()
            .push_int(2)
            .push_int(2)
            .push_opcode(OP_NUMNOTEQUAL)
            .into_script();
        let stack = vec![ScriptNum::from(0).to_bytes()].into();
        let result = EvalResult::ok(false, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_numnotequal_invalid_stack() {
        let script = Builder::default()
            .push_int(2)
            .push_opcode(OP_NUMNOTEQUAL)
            .into_script();
        let result = EvalResult::err(StackError::InvalidOperation);
        basic_test(&script, result);
    }

    #[test]
    fn test_lessthan() {
        let script = Builder::default()
            .push_int(2)
            .push_int(3)
            .push_opcode(OP_LESSTHAN)
            .into_script();
        let stack = vec![ScriptNum::from(1).to_bytes()].into();
        let result = EvalResult::ok(true, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_lessthan_not() {
        let script = Builder::default()
            .push_int(2)
            .push_int(2)
            .push_opcode(OP_LESSTHAN)
            .into_script();
        let stack = vec![ScriptNum::from(0).to_bytes()].into();
        let result = EvalResult::ok(false, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_lessthan_invalid_stack() {
        let script = Builder::default()
            .push_int(2)
            .push_opcode(OP_LESSTHAN)
            .into_script();
        let result = EvalResult::err(StackError::InvalidOperation);
        basic_test(&script, result);
    }

    #[test]
    fn test_greaterthan() {
        let script = Builder::default()
            .push_int(3)
            .push_int(2)
            .push_opcode(OP_GREATERTHAN)
            .into_script();
        let stack = vec![ScriptNum::from(1).to_bytes()].into();
        let result = EvalResult::ok(true, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_greaterthan_not() {
        let script = Builder::default()
            .push_int(2)
            .push_int(2)
            .push_opcode(OP_GREATERTHAN)
            .into_script();
        let stack = vec![ScriptNum::from(0).to_bytes()].into();
        let result = EvalResult::ok(false, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_greaterthan_invalid_stack() {
        let script = Builder::default()
            .push_int(2)
            .push_opcode(OP_GREATERTHAN)
            .into_script();
        let result = EvalResult::err(StackError::InvalidOperation);
        basic_test(&script, result);
    }

    #[test]
    fn test_lessthanorequal() {
        let script = Builder::default()
            .push_int(2)
            .push_int(3)
            .push_opcode(OP_LESSTHANOREQUAL)
            .into_script();
        let stack = vec![ScriptNum::from(1).to_bytes()].into();
        let result = EvalResult::ok(true, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_lessthanorequal_equal() {
        let script = Builder::default()
            .push_int(2)
            .push_int(2)
            .push_opcode(OP_LESSTHANOREQUAL)
            .into_script();
        let stack = vec![ScriptNum::from(1).to_bytes()].into();
        let result = EvalResult::ok(true, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_lessthanorequal_not() {
        let script = Builder::default()
            .push_int(2)
            .push_int(1)
            .push_opcode(OP_LESSTHANOREQUAL)
            .into_script();
        let stack = vec![ScriptNum::from(0).to_bytes()].into();
        let result = EvalResult::ok(false, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_lessthanorequal_invalid_stack() {
        let script = Builder::default()
            .push_int(2)
            .push_opcode(OP_LESSTHANOREQUAL)
            .into_script();
        let result = EvalResult::err(StackError::InvalidOperation);
        basic_test(&script, result);
    }

    #[test]
    fn test_greaterthanorequal() {
        let script = Builder::default()
            .push_int(3)
            .push_int(2)
            .push_opcode(OP_GREATERTHANOREQUAL)
            .into_script();
        let stack = vec![ScriptNum::from(1).to_bytes()].into();
        let result = EvalResult::ok(true, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_greaterthanorequal_equal() {
        let script = Builder::default()
            .push_int(2)
            .push_int(2)
            .push_opcode(OP_GREATERTHANOREQUAL)
            .into_script();
        let stack = vec![ScriptNum::from(1).to_bytes()].into();
        let result = EvalResult::ok(true, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_greaterthanorequal_not() {
        let script = Builder::default()
            .push_int(1)
            .push_int(2)
            .push_opcode(OP_GREATERTHANOREQUAL)
            .into_script();
        let stack = vec![ScriptNum::from(0).to_bytes()].into();
        let result = EvalResult::ok(false, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_greaterthanorequal_invalid_stack() {
        let script = Builder::default()
            .push_int(2)
            .push_opcode(OP_GREATERTHANOREQUAL)
            .into_script();
        let result = EvalResult::err(StackError::InvalidOperation);
        basic_test(&script, result);
    }

    #[test]
    fn test_min() {
        let script = Builder::default()
            .push_int(2)
            .push_int(3)
            .push_opcode(OP_MIN)
            .into_script();
        let stack = vec![ScriptNum::from(2).to_bytes()].into();
        let result = EvalResult::ok(true, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_min_second() {
        let script = Builder::default()
            .push_int(4)
            .push_int(3)
            .push_opcode(OP_MIN)
            .into_script();
        let stack = vec![ScriptNum::from(3).to_bytes()].into();
        let result = EvalResult::ok(true, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_min_invalid_stack() {
        let script = Builder::default()
            .push_int(4)
            .push_opcode(OP_MIN)
            .into_script();
        let result = EvalResult::err(StackError::InvalidOperation);
        basic_test(&script, result);
    }

    #[test]
    fn test_max() {
        let script = Builder::default()
            .push_int(2)
            .push_int(3)
            .push_opcode(OP_MAX)
            .into_script();
        let stack = vec![ScriptNum::from(3).to_bytes()].into();
        let result = EvalResult::ok(true, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_max_second() {
        let script = Builder::default()
            .push_int(4)
            .push_int(3)
            .push_opcode(OP_MAX)
            .into_script();
        let stack = vec![ScriptNum::from(4).to_bytes()].into();
        let result = EvalResult::ok(true, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_max_invalid_stack() {
        let script = Builder::default()
            .push_int(4)
            .push_opcode(OP_MAX)
            .into_script();
        let result = EvalResult::err(StackError::InvalidOperation);
        basic_test(&script, result);
    }

    #[test]
    fn test_within() {
        let script = Builder::default()
            .push_int(3)
            .push_int(2)
            .push_int(4)
            .push_opcode(OP_WITHIN)
            .into_script();
        let stack = vec![vec![1].into()].into();
        let result = EvalResult::ok(true, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_within_not() {
        let script = Builder::default()
            .push_int(3)
            .push_int(5)
            .push_int(4)
            .push_opcode(OP_WITHIN)
            .into_script();
        let stack = vec![Vec::new()].into();
        let result = EvalResult::ok(false, stack);
        basic_test(&script, result);
    }

    #[test]
    fn test_within_invalid_stack() {
        let script = Builder::default()
            .push_int(5)
            .push_int(4)
            .push_opcode(OP_WITHIN)
            .into_script();
        let result = EvalResult::err(StackError::InvalidOperation);
        basic_test(&script, result);
    }
}
