mod eval;
mod multisig;
mod sig;
// mod verify;

use crate::num::NumError;
use crate::stack::StackError;

pub use self::eval::eval_script;
// pub use self::verify::verify_script;

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
    #[error("signature push only")]
    SignaturePushOnly,
    #[error("witness malleated p2sh")]
    WitnessMalleatedP2SH,
    #[error("eval script false")]
    EvalFalse,
    #[error("Witness malleated")]
    WitnessMalleated,
    #[error("witness unexpected")]
    WitnessUnexpected,
    #[error("clean stack")]
    CleanStack,
    #[error("discourage upgradable witness program")]
    DiscourageUpgradableWitnessProgram,
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
