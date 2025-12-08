use bitcoin::consensus::encode::FromHexError;
use jsonrpsee::types::ErrorObjectOwned;
use jsonrpsee::types::error::ErrorObject;

/// Bitcoin Core compatible RPC errors.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Block not found")]
    BlockNotFound,

    #[error("Block height out of range")]
    BlockHeightOutOfRange,

    #[error("Invalid block hash")]
    InvalidBlockHash,

    #[error("Transaction not found")]
    TransactionNotFound,

    #[error("Invalid transaction hex")]
    InvalidTransactionHex,

    #[error("Bitcoin P2P network service unavailable")]
    NetworkUnavailable,

    #[error("Invalid header: {0:?}")]
    Header(subcoin_primitives::HeaderError),

    #[error(transparent)]
    BitcoinIO(#[from] bitcoin::io::Error),

    #[error(transparent)]
    Blockchain(#[from] sp_blockchain::Error),

    #[error(transparent)]
    DecodeHex(#[from] FromHexError),

    #[error(transparent)]
    SerdeJson(#[from] serde_json::Error),

    #[error("Client error: {0}")]
    Client(#[from] Box<dyn std::error::Error + Send + Sync>),

    #[error("{0}")]
    Other(String),
}

// Bitcoin Core RPC error codes
// See: https://github.com/bitcoin/bitcoin/blob/master/src/rpc/protocol.h
#[allow(dead_code)]
mod rpc_error_code {
    /// General error during transaction or block submission
    pub const RPC_VERIFY_ERROR: i32 = -25;
    /// Invalid address or key
    pub const RPC_INVALID_ADDRESS_OR_KEY: i32 = -5;
    /// Invalid parameter
    pub const RPC_INVALID_PARAMETER: i32 = -8;
    /// Database error
    pub const RPC_DATABASE_ERROR: i32 = -20;
    /// Error parsing or validating structure in raw format
    pub const RPC_DESERIALIZATION_ERROR: i32 = -22;
    /// General application-defined error
    pub const RPC_MISC_ERROR: i32 = -1;
}

impl From<Error> for ErrorObjectOwned {
    fn from(e: Error) -> ErrorObjectOwned {
        use rpc_error_code::*;

        match e {
            Error::BlockNotFound | Error::TransactionNotFound => {
                ErrorObject::owned(RPC_INVALID_ADDRESS_OR_KEY, e.to_string(), None::<()>)
            }
            Error::BlockHeightOutOfRange | Error::InvalidBlockHash => {
                ErrorObject::owned(RPC_INVALID_PARAMETER, e.to_string(), None::<()>)
            }
            Error::InvalidTransactionHex | Error::DecodeHex(_) => {
                ErrorObject::owned(RPC_DESERIALIZATION_ERROR, e.to_string(), None::<()>)
            }
            Error::Blockchain(_) => {
                ErrorObject::owned(RPC_DATABASE_ERROR, e.to_string(), None::<()>)
            }
            _ => ErrorObject::owned(RPC_MISC_ERROR, e.to_string(), None::<()>),
        }
    }
}
