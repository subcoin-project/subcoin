mod interpreter;
mod num;
mod stack;

use bitcoin::opcodes::Opcode;
use bitflags::bitflags;
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

impl VerificationFlags {
    pub fn verify_minimaldata(&self) -> bool {
        self.contains(Self::MINIMALDATA)
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
    #[error("{0} is disabled")]
    DisabledOpcode(Opcode),
    #[error("equal verify")]
    EqualVerify,
    #[error("script error: {0:?}")]
    Script(bitcoin::script::Error),
}

pub fn verify_script(left: u64, right: u64) -> Result<(), Error> {
    Ok(())
}
