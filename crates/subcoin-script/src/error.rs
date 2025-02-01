use crate::constants::{MAX_OPS_PER_SCRIPT, MAX_SCRIPT_ELEMENT_SIZE, MAX_STACK_SIZE};
use crate::interpreter::{CheckMultiSigError, CheckSigError};
use crate::num::NumError;
use crate::signature_checker::SignatureError;
use crate::stack::StackError;

/// Script error type.
#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum Error {
    ///////////////////////////
    // Script error.
    ///////////////////////////
    /// ErrEvalFalse is returned when the script evaluated without error but
    /// terminated with a false top stack element.
    #[error("script terminated with a false stack element")]
    EvalFalse,
    #[error("Return early if OP_RETURN is executed in the script.")]
    OpReturn,

    // Max sizes.
    #[error("Exceeds max script size")]
    ScriptSize,
    #[error("Size of the element pushed to the stack exceeds MAX_SCRIPT_ELEMENT_SIZE ({MAX_SCRIPT_ELEMENT_SIZE})")]
    PushSize,
    #[error("Exceeds max operations ({MAX_OPS_PER_SCRIPT}) per script")]
    OpCount,
    // Stack and altstack combined depth is over the limit.
    #[error("Exceeds stack limit ({MAX_STACK_SIZE})")]
    StackSize,
    #[error("sig count")]
    SigCount,
    #[error("pubkey count")]
    PubkeyCount,

    // Failed verify operations.
    #[error("OP_VERIFY failed at opcode: {0:?}")]
    Verify(bitcoin::opcodes::Opcode),

    // Logical/Format/Canonical errors.
    #[error("bad opcode")]
    BadOpcode,
    // A disabled opcode is encountered in a script.
    #[error("attempt to execute disabled opcode {0}")]
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
    // Script contains an opcode that interprets a negative lock time.
    #[error("Locktime is negative")]
    NegativeLocktime,
    // Script contains an opcode that involves a lock time and the required
    // lock time has not been reached.
    #[error("Required lock time has not been reached.")]
    UnsatisfiedLocktime,

    // Malleability
    #[error("sig hash type")]
    SigHashType,
    #[error("Native witness program cannot also have a signature script")]
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
    // ErrMinimalIf is returned if ScriptVerifyWitness is set and the
    // operand of an OP_IF/OP_NOF_IF are not either an empty vector or
    // [0x01].
    #[error("minimal if")]
    Minimalif,
    #[error("signature push only")]
    SigPushOnly,
    #[error("witness program witness empty")]
    WitnessProgramWitnessEmpty,
    #[error("witness program mismatch")]
    WitnessProgramMismatch,
    #[error("witness program wrong length")]
    WitnessProgramWrongLength,

    // Softfork safeness.
    #[error("NOP opcode encountered when DISCOURAGE_UPGRADABLE_NOPS flag is set")]
    DiscourageUpgradableNops,
    #[error("discourage upgradable witness program")]
    DiscourageUpgradableWitnessProgram,
    #[error("discourage upgradable taproot program")]
    DiscourageUpgradableTaprootProgram,
    #[error("discourage upgradable taproot version")]
    DiscourageUpgradableTaprootVersion,
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
    // OP_CODESEPARATOR is used in a non-segwit script.
    #[error("OP_CODESEPARATOR is used in a non-segwit script.")]
    OpCodeSeparator,
    #[error("sig findanddelete")]
    SigFindAndDelete,

    #[error("error count")]
    ErrorCount,

    // Extended errors.
    #[error("No script execution")]
    NoScriptExecution,
    #[error("Invalid alt stack operation")]
    InvalidAltStackOperation,
    #[error("{0} is unknown")]
    UnknownOpcode(bitcoin::opcodes::Opcode),
    #[error("Failed to read instruction: {0:?}")]
    ReadInstruction(bitcoin::script::Error),
    #[error(transparent)]
    Num(#[from] NumError),
    #[error("schnorr signature error: {0:?}")]
    SchnorrSignature(bitcoin::taproot::SigFromSliceError),
    #[error("taproot error: {0:?}")]
    Taproot(bitcoin::taproot::TaprootError),
    #[error("secp256k1 error: {0:?}")]
    Secp256k1(bitcoin::secp256k1::Error),
    #[error(transparent)]
    Signature(#[from] SignatureError),
    #[error(transparent)]
    CheckSig(#[from] CheckSigError),
    #[error(transparent)]
    CheckMultiSig(#[from] CheckMultiSigError),
}
