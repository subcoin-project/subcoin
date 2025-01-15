mod num;
mod stack;

use bitcoin::opcodes::all::*;
use bitcoin::opcodes::Opcode;
use bitcoin::script::Instruction;
use bitcoin::{Script, Witness};
use bitflags::bitflags;
use num::ScriptNum;
use primitive_types::H256;

bitflags! {
    /// Script verification flags.
    ///
    /// https://github.com/bitcoin/bitcoin/blob/6f9db1ebcab4064065ccd787161bf2b87e03cc1f/src/script/interpreter.h#L45
    pub struct VerificationFlags: u32 {
        const NONE = 0;
        const P2SH = 1 << 0;
        const STRICTENC = 1 << 1;
        const DERSIG = 1 << 2;
        const LOW_S = 1 << 3;
        const NULLDUMMY = 1 << 4;
        const SIGPUSHONLY = 1 << 5;
        const MINIMALDATA = 1 << 6;
        const DISCOURAGE_UPGRADABLE_NOPS = 1 << 7;
        const CLEANSTACK = 1 << 8;
        const CHECKLOCKTIMEVERIFY = 1 << 9;
        const CHECKSEQUENCEVERIFY = 1 << 10;
        const WITNESS = 1 << 11;
        const DISCOURAGE_UPGRADABLE_WITNESS_PROGRAM = 1 << 12;
        const MINIMALIF = 1 << 13;
        const NULLFAIL = 1 << 14;
        const WITNESS_PUBKEYTYPE = 1 << 15;
        const CONST_SCRIPTCODE = 1 << 16;
        const TAPROOT = 1 << 17;
        const DISCOURAGE_UPGRADABLE_TAPROOT_VERSION = 1 << 18;
        const DISCOURAGE_OP_SUCCESS = 1 << 19;
        const DISCOURAGE_UPGRADABLE_PUBKEYTYPE = 1 << 20;
    }
}

/// Represents different signature verification schemes used in Bitcoin
///
/// https://github.com/bitcoin/bitcoin/blob/6f9db1ebcab4064065ccd787161bf2b87e03cc1f/src/script/interpreter.h#L190
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigVersion {
    /// Bare scripts and BIP16 P2SH-wrapped redeemscripts
    Base,
    /// Witness v0 (P2WPKH and P2WSH); see BIP 141
    WitnessV0,
    /// Witness v1 with 32-byte program, not BIP16 P2SH-wrapped, key path spending; see BIP 341
    Taproot,
    /// Witness v1 with 32-byte program, not BIP16 P2SH-wrapped, script path spending,
    /// leaf version 0xc0; see BIP 342
    Tapscript,
}

// https://github.com/bitcoin/bitcoin/blob/6f9db1ebcab4064065ccd787161bf2b87e03cc1f/src/script/interpreter.h#L198
#[derive(Default)]
pub struct ScriptExecutionData {
    /// Whether m_tapleaf_hash is initialized
    pub tapleaf_hash_init: bool,
    /// The tapleaf hash
    pub tapleaf_hash: H256,

    /// Whether m_codeseparator_pos is initialized
    pub codeseparator_pos_init: bool,
    /// Opcode position of the last executed OP_CODESEPARATOR (or 0xFFFFFFFF if none executed)
    pub codeseparator_pos: u32,

    /// Whether m_annex_present and m_annex_hash are initialized
    pub annex_init: bool,
    /// Whether an annex is present
    pub annex_present: bool,
    /// Hash of the annex data
    pub annex_hash: H256,

    /// Whether m_validation_weight_left is initialized
    pub validation_weight_left_init: bool,
    /// How much validation weight is left (decremented for every successful non-empty signature check)
    pub validation_weight_left: i64,

    /// The hash of the corresponding output
    pub output_hash: Option<H256>,
}

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum Error {
    #[error("Script number overflow")]
    NumberOverflow,
    #[error("Non-minimally encoded script number")]
    NumberNotMinimallyEncoded,
    #[error("invalid stack operation")]
    InvalidStackOperation,
    #[error("script error: {0:?}")]
    Script(bitcoin::script::Error),
}

pub fn verify_script(left: u64, right: u64) -> Result<(), Error> {
    Ok(())
}

pub fn eval_script(
    stack: &mut Vec<Vec<u8>>,
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
