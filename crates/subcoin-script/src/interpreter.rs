mod constants;
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
    /// ErrEvalFalse is returned when the script evaluated without error but
    /// terminated with a false top stack element.
    #[error("script terminated with a false stack element")]
    EvalFalse,
    #[error("op return")]
    OpReturn,

    // Max sizes.
    #[error("script size")]
    ScriptSize,
    #[error("push size")]
    PushSize,
    #[error("Exceeds max ops per script")]
    OpCount,
    // Stack and altstack combined depth is over the limit.
    #[error("stack overflow")]
    ExceedsStackLimit,
    #[error("sig count")]
    SigCount,
    #[error("pubkey count")]
    PubkeyCount,

    // Failed verify operations.
    #[error("failed verify operation: {0:?}")]
    Verify(bitcoin::opcodes::Opcode),

    // Logical/Format/Canonical errors.
    #[error("bad opcode")]
    BadOpcode,
    // ErrDisabledOpcode is returned when a disabled opcode is encountered
    // in a script.
    #[error("{0} is disabled")]
    DisabledOpcode(bitcoin::opcodes::Opcode),
    #[error(transparent)]
    Stack(#[from] StackError),
    // ErrUnbalancedConditional is returned when an OP_ELSE or OP_ENDIF is
    // encountered in a script without first having an OP_IF or OP_NOTIF or
    // the end of script is reached without encountering an OP_ENDIF when
    // an OP_IF or OP_NOTIF was previously encountered.
    #[error("unbalanced conditional")]
    UnbalancedConditional,

    // CHECKLOCKTIMEVERIFY and CHECKSEQUENCEVERIFY
    // ErrNegativeLockTime is returned when a script contains an opcode that
    // interprets a negative lock time.
    #[error("negative locktime")]
    NegativeLocktime,
    // ErrUnsatisfiedLockTime is returned when a script contains an opcode
    // that involves a lock time and the required lock time has not been
    // reached.
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
    // ErrCleanStack is returned when the ScriptVerifyCleanStack flag
    // is set, and after evaluation, the stack does not contain only a
    // single element.
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
    // ErrDiscourageOpSuccess is returned if
    // ScriptVerifyDiscourageOpSuccess is active, and a OP_SUCCESS op code
    // is encountered during tapscript validation.
    #[error("OP_SUCCESS encountered when ScriptVerifyDiscourageOpSuccess is active.")]
    DiscourageOpSuccess,
    // ErrDiscourageUpgradeableTaprootVersion is returned if
    // ScriptVerifyDiscourageUpgradeableTaprootVersion is active and a leaf
    // version encountered isn't the base leaf version.
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
    // ErrMinimalIf is returned if ScriptVerifyWitness is set and the
    // operand of an OP_IF/OP_NOF_IF are not either an empty vector or
    // [0x01].
    #[error("taproot minimalif")]
    TaprootMinimalif,

    // Constant scriptCode
    // ErrCodeSeparator is returned when OP_CODESEPARATOR is used in a
    // non-segwit script.
    #[error("OP_CODESEPARATOR is used in a non-segwit script.")]
    OpCodeSeparator,
    #[error("sig findanddelete")]
    SigFindAndDelete,

    #[error("error count")]
    ErrorCount,

    // Extended errors.
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
