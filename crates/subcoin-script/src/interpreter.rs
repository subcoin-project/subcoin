mod eval;
mod verify;

use crate::num::NumError;
use crate::stack::StackError;

pub use self::eval::eval_script;
pub use self::verify::verify_script;

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum ScriptError {
    ///////////////////////////
    // Script error.
    ///////////////////////////
    #[error("eval script false")]
    EvalFalse,
    #[error("op return")]
    OpReturn,

    // Max sizes.
    #[error("script size")]
    ScriptSize,
    #[error("push size")]
    PushSize,
    #[error("op count")]
    OpCount,
    #[error("stack size")]
    StackSize,
    #[error("sig count")]
    SigCount,
    #[error("pubkey count")]
    PubkeyCount,

    // Failed verify operations.
    #[error("failed verify operation: {0:?}")]
    FailedVerify(bitcoin::opcodes::Opcode),

    // Logical/Format/Canonical errors.
    #[error("bad opcode")]
    BadOpcode,
    #[error("{0} is disabled")]
    DisabledOpcode(bitcoin::opcodes::Opcode),
    #[error(transparent)]
    Stack(#[from] StackError),
    #[error("unbalanced conditional")]
    UnbalancedConditional,

    // CHECKLOCKTIMEVERIFY and CHECKSEQUENCEVERIFY
    #[error("negative locktime")]
    NegativeLocktime,
    #[error("unsatisfied locktime")]
    UnsatisfiedLocktime,

    // Malleability
    #[error("sig hash type")]
    SigHashType,
    #[error("Witness malleated")]
    WitnessMalleated,
    #[error("witness unexpected")]
    WitnessUnexpected,
    #[error("witness malleated p2sh")]
    WitnessMalleatedP2SH,
    #[error("clean stack")]
    CleanStack,
    #[error("signature push only")]
    SigPushOnly,
    #[error("witness program witness empty")]
    WitnessProgramWitnessEmpty,
    #[error("witness program mismatch")]
    WitnessProgramMismatch,
    #[error("witness program wrong length")]
    WitnessProgramWrongLength,

    // Softfork safeness.
    #[error("disable upgrable nops")]
    DiscourageUpgradableNops,
    #[error("discourage upgradable witness program")]
    DiscourageUpgradableWitnessProgram,
    #[error("discourage upgradable taproot program")]
    DiscourageUpgradableTaprootProgram,
    #[error("discourage op success")]
    DiscourageOpSuccess,
    #[error("discourage upgrable pubkey type")]
    DiscourageUpgradablePubkeyType,

    // Taproot
    #[error("schnorr sig size")]
    SchnorrSigSize,
    #[error("schnorr sig hash type")]
    SchnorrSigHashType,
    #[error("schnorr sig")]
    SchnorrSig,
    #[error("taproot wrong control size")]
    TaprootWrongControlSize,
    #[error("taproot validation weight")]
    TaprootValidationWeight,
    #[error("taproot checkmultisig")]
    TaprootCheckmultisig,
    #[error("taproot minimalif")]
    TaprootMinimalif,

    // Constant scriptCode
    #[error("op codeseparator")]
    OpCodeSeparator,
    #[error("sig findanddelete")]
    SigFindAndDelete,

    #[error("error count")]
    ErrorCount,

    // Extended errors.
    #[error("return opcode")]
    ReturnOpcode,
    #[error("invalid alt stack operation")]
    InvalidAltStackOperation,
    #[error("{0} is unknown")]
    UnknownOpcode(bitcoin::opcodes::Opcode),
    #[error("rust-bitcoin script error: {0:?}")]
    RustBitcoinScript(bitcoin::script::Error),
    #[error(transparent)]
    Num(#[from] NumError),
    #[error(transparent)]
    EvalScript(#[from] eval::EvalScriptError),
}

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum Error {
    #[error("witness malleated p2sh")]
    WitnessMalleatedP2SH,
    #[error("invalid script: {0:?}")]
    Script(#[from] ScriptError),
}

type Result<T> = std::result::Result<T, Error>;
