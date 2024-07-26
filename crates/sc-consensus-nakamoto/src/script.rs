mod bytes;
mod flags;
mod stack;

use bitcoin::Opcode;

/// Maximum number of bytes pushable to the stack
pub const MAX_SCRIPT_ELEMENT_SIZE: usize = 520;

/// Maximum number of non-push operations per script
pub const MAX_OPS_PER_SCRIPT: u32 = 201;

/// Maximum number of public keys per multisig
pub const MAX_PUBKEYS_PER_MULTISIG: usize = 20;

/// Maximum script length in bytes
pub const MAX_SCRIPT_SIZE: usize = 10000;

// Below flags apply in the context of BIP 68
// If this flag set, CTxIn::nSequence is NOT interpreted as a
// relative lock-time.
pub const SEQUENCE_LOCKTIME_DISABLE_FLAG: u32 = 1u32 << 31;

/// Bitcoin script interpreter errors.
#[derive(Debug, PartialEq, thiserror::Error)]
pub enum Error {
    #[error("Unknown error")]
    Unknown,
    #[error("Script evaluated to false")]
    EvalFalse,
    #[error("Used return opcode")]
    ReturnOpcode,

    // Max sizes.
    #[error("Script is too long")]
    ScriptSize,
    #[error("Pushing too many bytes")]
    PushSize,
    #[error("Script contains to many opcodes")]
    OpCount,
    #[error("Stack is too big")]
    StackSize,
    #[error("Number overflow")]
    NumberOverflow,
    #[error("Number not minimally encoded")]
    NumberNotMinimallyEncoded,
    #[error("Maximum number of signature exceeded")]
    SigCount,
    #[error("Maximum number of pubkeys per multisig exceeded")]
    PubkeyCount,
    #[error("Invalid operand size")]
    InvalidOperandSize,

    // Failed verify operations
    #[error("Failed verify operation")]
    Verify,
    #[error("Failed equal verify operation")]
    EqualVerify,
    #[error("Failed signature check")]
    CheckSigVerify,
    #[error("Failed data signature check")]
    CheckDataSigVerify,
    #[error("Failed num equal verify operation")]
    NumEqualVerify,

    // Logical/Format/Canonical errors.
    #[error("Bad Opcode")]
    BadOpcode,
    #[error("Disabled Opcode: {0:?}")]
    DisabledOpcode(Opcode),
    #[error("Invalid stack operation")]
    InvalidStackOperation,
    #[error("Invalid altstack operation")]
    InvalidAltstackOperation,
    #[error("Unbalanced conditional")]
    UnbalancedConditional,
    #[error("Invalid OP_SPLIT range")]
    InvalidSplitRange,
    #[error("Invalid division operation")]
    DivisionByZero,
    #[error("The requested encoding is impossible to satisfy")]
    ImpossibleEncoding,

    // CHECKLOCKTIMEVERIFY and CHECKSEQUENCEVERIFY
    #[error("Negative locktime")]
    NegativeLocktime,
    #[error("UnsatisfiedLocktime")]
    UnsatisfiedLocktime,

    // BIP62
    #[error("Invalid Signature Hashtype")]
    SignatureHashtype,
    #[error("Invalid Signature")]
    SignatureDer,
    #[error("Illegal use of SIGHASH_FORKID")]
    SignatureIllegalForkId,
    #[error("Signature must use SIGHASH_FORKID")]
    SignatureMustUseForkId,
    #[error("Check minimaldata failed")]
    Minimaldata,
    #[error("Only push opcodes are allowed in this signature")]
    SignaturePushOnly,
    #[error("Invalid High S in Signature")]
    SignatureHighS,
    #[error("Multisig extra stack element is not empty")]
    SignatureNullDummy,
    #[error("Invalid Pubkey")]
    PubkeyType,
    #[error("Only one element is expected to remain at stack at the end of execution")]
    Cleanstack,

    // Softfork safeness
    #[error("Discourage Upgradable Nops")]
    DiscourageUpgradableNops,
    #[error("Discourage Upgradable Witness Program")]
    DiscourageUpgradableWitnessProgram,

    // SegWit-related errors
    #[error("Witness program has incorrect length")]
    WitnessProgramWrongLength,
    #[error("Witness program was passed an empty witness")]
    WitnessProgramWitnessEmpty,
    #[error("Witness program hash mismatch")]
    WitnessProgramMismatch,
    #[error("Witness requires empty scriptSig")]
    WitnessMalleated,
    #[error("Witness requires only-redeemscript scriptSig")]
    WitnessMalleatedP2SH,
    #[error("Witness provided for non-witness script")]
    WitnessUnexpected,
    #[error("Using non-compressed keys in segwit")]
    WitnessPubKeyType,
}
