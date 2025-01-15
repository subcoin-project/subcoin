use crate::num::ScriptNum;
use crate::stack::Stack;
use crate::{Error, ScriptExecutionData, SigVersion, VerificationFlags};
use bitcoin::opcodes::all::*;
use bitcoin::opcodes::Opcode;
use bitcoin::script::Instruction;
use bitcoin::{Script, Witness};
use primitive_types::H256;
use std::ops::Add;
use std::ops::Neg;
use std::ops::Sub;

pub fn eval_script(
    stack: &mut Stack,
    script: &Script,
    flags: VerificationFlags,
    sig_version: SigVersion,
    exec_data: &mut ScriptExecutionData,
) -> Result<(), Error> {
    // Create an alternate stack
    // let mut alt_stack = Vec::new();

    // Create a vector of conditional execution states
    // let mut exec_stack = Vec::new();
    // let mut in_exec = true;

    let verify_minimaldata = flags.verify_minimaldata();

    for instruction in script.instructions() {
        let instruction = instruction.map_err(Error::Script)?;

        match instruction {
            Instruction::PushBytes(p) => {
                stack.push(p.as_bytes().to_vec());
            }
            Instruction::Op(opcode) => match opcode {
                OP_PUSHDATA1 | OP_PUSHDATA2 | OP_PUSHDATA4 | OP_PUSHBYTES_0 | OP_PUSHBYTES_1
                | OP_PUSHBYTES_2 | OP_PUSHBYTES_3 | OP_PUSHBYTES_4 | OP_PUSHBYTES_5
                | OP_PUSHBYTES_6 | OP_PUSHBYTES_7 | OP_PUSHBYTES_8 | OP_PUSHBYTES_9
                | OP_PUSHBYTES_10 | OP_PUSHBYTES_11 | OP_PUSHBYTES_12 | OP_PUSHBYTES_13
                | OP_PUSHBYTES_14 | OP_PUSHBYTES_15 | OP_PUSHBYTES_16 | OP_PUSHBYTES_17
                | OP_PUSHBYTES_18 | OP_PUSHBYTES_19 | OP_PUSHBYTES_20 | OP_PUSHBYTES_21
                | OP_PUSHBYTES_22 | OP_PUSHBYTES_23 | OP_PUSHBYTES_24 | OP_PUSHBYTES_25
                | OP_PUSHBYTES_26 | OP_PUSHBYTES_27 | OP_PUSHBYTES_28 | OP_PUSHBYTES_29
                | OP_PUSHBYTES_30 | OP_PUSHBYTES_31 | OP_PUSHBYTES_32 | OP_PUSHBYTES_33
                | OP_PUSHBYTES_34 | OP_PUSHBYTES_35 | OP_PUSHBYTES_36 | OP_PUSHBYTES_37
                | OP_PUSHBYTES_38 | OP_PUSHBYTES_39 | OP_PUSHBYTES_40 | OP_PUSHBYTES_41
                | OP_PUSHBYTES_42 | OP_PUSHBYTES_43 | OP_PUSHBYTES_44 | OP_PUSHBYTES_45
                | OP_PUSHBYTES_46 | OP_PUSHBYTES_47 | OP_PUSHBYTES_48 | OP_PUSHBYTES_49
                | OP_PUSHBYTES_50 | OP_PUSHBYTES_51 | OP_PUSHBYTES_52 | OP_PUSHBYTES_53
                | OP_PUSHBYTES_54 | OP_PUSHBYTES_55 | OP_PUSHBYTES_56 | OP_PUSHBYTES_57
                | OP_PUSHBYTES_58 | OP_PUSHBYTES_59 | OP_PUSHBYTES_60 | OP_PUSHBYTES_61
                | OP_PUSHBYTES_62 | OP_PUSHBYTES_63 | OP_PUSHBYTES_64 | OP_PUSHBYTES_65
                | OP_PUSHBYTES_66 | OP_PUSHBYTES_67 | OP_PUSHBYTES_68 | OP_PUSHBYTES_69
                | OP_PUSHBYTES_70 | OP_PUSHBYTES_71 | OP_PUSHBYTES_72 | OP_PUSHBYTES_73
                | OP_PUSHBYTES_74 | OP_PUSHBYTES_75 => {
                    unreachable!("Instruction::Op(opcode) contains non-push opcode only");
                }
                OP_CAT | OP_SUBSTR | OP_LEFT | OP_RIGHT | OP_INVERT | OP_AND | OP_OR | OP_XOR
                | OP_2MUL | OP_2DIV | OP_MUL | OP_DIV | OP_MOD | OP_LSHIFT | OP_RSHIFT => {
                    return Err(Error::DisabledOpcode(opcode));
                }
                OP_EQUAL => {
                    let v1 = stack.pop()?;
                    let v2 = stack.pop()?;
                    if v1 == v2 {
                        stack.push(vec![1]);
                    } else {
                        stack.push(Vec::new());
                    }
                }
                OP_EQUALVERIFY => {
                    let equal = stack.pop()? == stack.pop()?;
                    if !equal {
                        return Err(Error::EqualVerify);
                    }
                }
                OP_SIZE => {
                    let n = ScriptNum::from(stack.last()?.len() as i64);
                    stack.push(n.to_bytes());
                }
                OP_1ADD => {
                    let n = ScriptNum::from_bytes(&stack.pop()?, verify_minimaldata, None)?
                        .add(1.into())?;
                    stack.push(n.to_bytes());
                }
                OP_1SUB => {
                    let n = ScriptNum::from_bytes(&stack.pop()?, verify_minimaldata, None)?
                        .sub(1.into())?;
                    stack.push(n.to_bytes());
                }
                OP_NEGATE => {
                    let n =
                        ScriptNum::from_bytes(&stack.pop()?, verify_minimaldata, None)?.neg()?;
                    stack.push(n.to_bytes());
                }
                OP_ABS => {
                    let n = ScriptNum::from_bytes(&stack.pop()?, verify_minimaldata, None)?.abs();
                    stack.push(n.to_bytes());
                }
                OP_NOT => {
                    let n =
                        ScriptNum::from_bytes(&stack.pop()?, verify_minimaldata, None)?.is_zero();
                    let n = ScriptNum::from(n);
                    stack.push(n.to_bytes());
                }
                OP_0NOTEQUAL => {
                    let n =
                        !ScriptNum::from_bytes(&stack.pop()?, verify_minimaldata, None)?.is_zero();
                    let n = ScriptNum::from(n);
                    stack.push(n.to_bytes());
                }
                OP_ADD => {
                    let v1 = ScriptNum::from_bytes(&stack.pop()?, verify_minimaldata, None)?;
                    let v2 = ScriptNum::from_bytes(&stack.pop()?, verify_minimaldata, None)?;
                    stack.push((v1 + v2)?.to_bytes());
                }
                OP_SUB => {
                    let v1 = ScriptNum::from_bytes(&stack.pop()?, verify_minimaldata, None)?;
                    let v2 = ScriptNum::from_bytes(&stack.pop()?, verify_minimaldata, None)?;
                    stack.push((v2 - v1)?.to_bytes());
                }
                OP_BOOLAND => {
                    let v1 = ScriptNum::from_bytes(&stack.pop()?, verify_minimaldata, None)?;
                    let v2 = ScriptNum::from_bytes(&stack.pop()?, verify_minimaldata, None)?;
                    let v = ScriptNum::from(!v1.is_zero() && !v2.is_zero());
                    stack.push(v.to_bytes());
                }
                OP_BOOLOR => {
                    let v1 = ScriptNum::from_bytes(&stack.pop()?, verify_minimaldata, None)?;
                    let v2 = ScriptNum::from_bytes(&stack.pop()?, verify_minimaldata, None)?;
                    let v = ScriptNum::from(!v1.is_zero() || !v2.is_zero());
                    stack.push(v.to_bytes());
                }
                OP_NUMEQUAL => {
                    let v1 = ScriptNum::from_bytes(&stack.pop()?, verify_minimaldata, None)?;
                    let v2 = ScriptNum::from_bytes(&stack.pop()?, verify_minimaldata, None)?;
                    let v = ScriptNum::from(v1 == v2);
                    stack.push(v.to_bytes());
                }
                OP_NUMEQUALVERIFY => {
                    let v1 = ScriptNum::from_bytes(&stack.pop()?, verify_minimaldata, None)?;
                    let v2 = ScriptNum::from_bytes(&stack.pop()?, verify_minimaldata, None)?;
                    if v1 != v2 {
                        return Err(Error::EqualVerify);
                    }
                }
                OP_NUMNOTEQUAL => {
                    let v1 = ScriptNum::from_bytes(&stack.pop()?, verify_minimaldata, None)?;
                    let v2 = ScriptNum::from_bytes(&stack.pop()?, verify_minimaldata, None)?;
                    let v = ScriptNum::from(v1 != v2);
                    stack.push(v.to_bytes());
                }
                OP_LESSTHAN => {
                    let v1 = ScriptNum::from_bytes(&stack.pop()?, verify_minimaldata, None)?;
                    let v2 = ScriptNum::from_bytes(&stack.pop()?, verify_minimaldata, None)?;
                    let v = ScriptNum::from(v1 > v2);
                    stack.push(v.to_bytes());
                }
                OP_GREATERTHAN => {
                    let v1 = ScriptNum::from_bytes(&stack.pop()?, verify_minimaldata, None)?;
                    let v2 = ScriptNum::from_bytes(&stack.pop()?, verify_minimaldata, None)?;
                    let v = ScriptNum::from(v1 < v2);
                    stack.push(v.to_bytes());
                }
                OP_LESSTHANOREQUAL => {
                    let v1 = ScriptNum::from_bytes(&stack.pop()?, verify_minimaldata, None)?;
                    let v2 = ScriptNum::from_bytes(&stack.pop()?, verify_minimaldata, None)?;
                    let v = ScriptNum::from(v1 >= v2);
                    stack.push(v.to_bytes());
                }
                OP_GREATERTHANOREQUAL => {
                    let v1 = ScriptNum::from_bytes(&stack.pop()?, verify_minimaldata, None)?;
                    let v2 = ScriptNum::from_bytes(&stack.pop()?, verify_minimaldata, None)?;
                    let v = ScriptNum::from(v1 <= v2);
                    stack.push(v.to_bytes());
                }
                OP_MIN => {
                    let v1 = ScriptNum::from_bytes(&stack.pop()?, verify_minimaldata, None)?;
                    let v2 = ScriptNum::from_bytes(&stack.pop()?, verify_minimaldata, None)?;
                    stack.push(std::cmp::min(v1, v2).to_bytes());
                }
                OP_MAX => {
                    let v1 = ScriptNum::from_bytes(&stack.pop()?, verify_minimaldata, None)?;
                    let v2 = ScriptNum::from_bytes(&stack.pop()?, verify_minimaldata, None)?;
                    stack.push(std::cmp::max(v1, v2).to_bytes());
                }
                OP_WITHIN => {
                    let v1 = ScriptNum::from_bytes(&stack.pop()?, verify_minimaldata, None)?;
                    let v2 = ScriptNum::from_bytes(&stack.pop()?, verify_minimaldata, None)?;
                    let v3 = ScriptNum::from_bytes(&stack.pop()?, verify_minimaldata, None)?;
                    if v2 <= v3 && v3 < v1 {
                        stack.push(vec![1].into());
                    } else {
                        stack.push(Vec::new());
                    }
                }
                OP_NOP => {}
                OP_PUSHNUM_NEG1 | OP_PUSHNUM_1 | OP_PUSHNUM_2 | OP_PUSHNUM_3 | OP_PUSHNUM_4
                | OP_PUSHNUM_5 | OP_PUSHNUM_6 | OP_PUSHNUM_7 | OP_PUSHNUM_8 | OP_PUSHNUM_9
                | OP_PUSHNUM_10 | OP_PUSHNUM_11 | OP_PUSHNUM_12 | OP_PUSHNUM_13 | OP_PUSHNUM_14
                | OP_PUSHNUM_15 | OP_PUSHNUM_16 => {
                    let value =
                        (opcode.to_u8() as i32).wrapping_sub(OP_PUSHNUM_1.to_u8() as i32 - 1);
                    stack.push(ScriptNum::from(value as i64).to_bytes());
                }
                _ => {}
            },
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
}
