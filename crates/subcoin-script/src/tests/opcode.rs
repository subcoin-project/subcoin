use crate::interpreter::eval_script;
use crate::num::ScriptNum;
use crate::signature_checker::NoSignatureCheck;
use crate::stack::{Stack, StackError};
use crate::{Error, ScriptExecutionData, SigVersion, VerifyFlags};
use bitcoin::hex::FromHex;
use bitcoin::opcodes::all::*;
use bitcoin::script::{Builder, Script};

struct EvalResult {
    /// Result of [`eval_script`].
    result: Result<bool, Error>,
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

    let flags = VerifyFlags::P2SH;
    let version = SigVersion::Base;
    let mut checker = NoSignatureCheck;
    let mut stack = Stack::default();
    let eval_script_result = eval_script(
        &mut stack,
        script,
        &flags,
        &mut checker,
        version,
        &mut ScriptExecutionData::default(),
    );
    assert_eq!(eval_script_result, expected);
    if expected.is_ok() {
        let expected_stack =
            expected_stack.expect("Expected stack must be Some if eval result is ok");
        assert_eq!(stack, expected_stack);
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
    let result = EvalResult::err(Error::Verify(OP_EQUALVERIFY));
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
    let stack = vec![ScriptNum::from(6).as_bytes()].into();
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
    let stack = vec![ScriptNum::from(4).as_bytes()].into();
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
    let stack = vec![ScriptNum::from(-5).as_bytes()].into();
    let result = EvalResult::ok(true, stack);
    basic_test(&script, result);
}

#[test]
fn test_negate_negative() {
    let script = Builder::default()
        .push_int(-5)
        .push_opcode(OP_NEGATE)
        .into_script();
    let stack = vec![ScriptNum::from(5).as_bytes()].into();
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
    let stack = vec![ScriptNum::from(5).as_bytes()].into();
    let result = EvalResult::ok(true, stack);
    basic_test(&script, result);
}

#[test]
fn test_abs_negative() {
    let script = Builder::default()
        .push_int(-5)
        .push_opcode(OP_ABS)
        .into_script();
    let stack = vec![ScriptNum::from(5).as_bytes()].into();
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
    let stack = vec![ScriptNum::from(0).as_bytes()].into();
    let result = EvalResult::ok(false, stack);
    basic_test(&script, result);
}

#[test]
fn test_not_zero() {
    let script = Builder::default()
        .push_int(0.into())
        .push_opcode(OP_NOT)
        .into_script();
    let stack = vec![ScriptNum::from(1).as_bytes()].into();
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
    let stack = vec![ScriptNum::from(1).as_bytes()].into();
    let result = EvalResult::ok(true, stack);
    basic_test(&script, result);
}

#[test]
fn test_0notequal_zero() {
    let script = Builder::default()
        .push_int(0)
        .push_opcode(OP_0NOTEQUAL)
        .into_script();
    let stack = vec![ScriptNum::from(0).as_bytes()].into();
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
    let stack = vec![ScriptNum::from(5).as_bytes()].into();
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
    let stack = vec![ScriptNum::from(1).as_bytes()].into();
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
    let stack = vec![ScriptNum::from(1).as_bytes()].into();
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
    let stack = vec![ScriptNum::from(0).as_bytes()].into();
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
    let stack = vec![ScriptNum::from(0).as_bytes()].into();
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
    let stack = vec![ScriptNum::from(0).as_bytes()].into();
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
    let stack = vec![ScriptNum::from(1).as_bytes()].into();
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
    let stack = vec![ScriptNum::from(1).as_bytes()].into();
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
    let stack = vec![ScriptNum::from(1).as_bytes()].into();
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
    let stack = vec![ScriptNum::from(0).as_bytes()].into();
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
    let stack = vec![ScriptNum::from(1).as_bytes()].into();
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
    let stack = vec![ScriptNum::from(0).as_bytes()].into();
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
    let result = EvalResult::err(Error::Verify(OP_NUMEQUALVERIFY));
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
    let stack = vec![ScriptNum::from(1).as_bytes()].into();
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
    let stack = vec![ScriptNum::from(0).as_bytes()].into();
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
    let stack = vec![ScriptNum::from(1).as_bytes()].into();
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
    let stack = vec![ScriptNum::from(0).as_bytes()].into();
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
    let stack = vec![ScriptNum::from(1).as_bytes()].into();
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
    let stack = vec![ScriptNum::from(0).as_bytes()].into();
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
    let stack = vec![ScriptNum::from(1).as_bytes()].into();
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
    let stack = vec![ScriptNum::from(1).as_bytes()].into();
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
    let stack = vec![ScriptNum::from(0).as_bytes()].into();
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
    let stack = vec![ScriptNum::from(1).as_bytes()].into();
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
    let stack = vec![ScriptNum::from(1).as_bytes()].into();
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
    let stack = vec![ScriptNum::from(0).as_bytes()].into();
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
    let stack = vec![ScriptNum::from(2).as_bytes()].into();
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
    let stack = vec![ScriptNum::from(3).as_bytes()].into();
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
    let stack = vec![ScriptNum::from(3).as_bytes()].into();
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
    let stack = vec![ScriptNum::from(4).as_bytes()].into();
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
