use bitcoin::Txid;

/// Errors that can occur when validating or managing mempool transactions.
#[derive(Debug, thiserror::Error)]
pub enum MempoolError {
    #[error("Transaction already in mempool")]
    AlreadyInMempool,

    #[error("Coinbase transaction not allowed")]
    Coinbase,

    #[error("Missing inputs: {parents:?}")]
    MissingInputs { parents: Vec<Txid> },

    #[error("Fee rate {actual_kvb} sat/kvB too low (min: {min_kvb})")]
    FeeTooLow { min_kvb: u64, actual_kvb: u64 },

    #[error("Invalid fee rate calculation: {0}")]
    InvalidFeeRate(String),

    #[error("Too many sigops: {0}")]
    TooManySigops(i64),

    #[error("Too many ancestors: {0}")]
    TooManyAncestors(usize),

    #[error("Ancestor size too large: {0}")]
    AncestorSizeTooLarge(i64),

    #[error("Too many descendants: {0}")]
    TooManyDescendants(usize),

    #[error("Descendant size too large: {0}")]
    DescendantSizeTooLarge(i64),

    #[error("Not standard: {0}")]
    NotStandard(String),

    #[error("Transaction version not standard")]
    TxVersionNotStandard,

    #[error("Transaction size too small")]
    TxSizeTooSmall,

    #[error("Non-final transaction")]
    NonFinal,

    #[error("Non-BIP68-final")]
    NonBIP68Final,

    #[error("Negative fee")]
    NegativeFee,

    #[error("Overflow in fee calculation")]
    FeeOverflow,

    #[error("Mempool is full")]
    MempoolFull,

    #[error("Transaction conflicts with mempool: {0}")]
    TxConflict(String),

    #[error("Script validation failed: {0}")]
    ScriptValidationFailed(String),

    #[error("No conflicting transaction to replace")]
    NoConflictToReplace,

    #[error("Conflicting transaction is not replaceable (doesn't signal BIP125)")]
    TxNotReplaceable,

    #[error("Too many transactions to replace: {0} (max 100)")]
    TooManyReplacements(usize),

    #[error("Replacement introduces new unconfirmed inputs")]
    NewUnconfirmedInput,

    #[error("Missing conflict transaction in mempool")]
    MissingConflict,

    #[error("Insufficient fee: {0}")]
    InsufficientFee(String),

    #[error("Package too large: {0} transactions (max {1})")]
    PackageTooLarge(usize, usize),

    #[error("Package exceeds size limit: {0} vbytes")]
    PackageSizeTooLarge(u64),

    #[error("Package has cyclic dependencies")]
    PackageCyclicDependencies,

    #[error("Package feerate too low: {0}")]
    PackageFeeTooLow(String),

    #[error("Package validation failed for tx {0}: {1}")]
    PackageTxValidationFailed(bitcoin::Txid, String),

    #[error("Package relay is disabled")]
    PackageRelayDisabled,

    #[error(transparent)]
    TxError(#[from] subcoin_primitives::consensus::TxError),

    #[error("Runtime API error: {0}")]
    RuntimeApi(String),
}
