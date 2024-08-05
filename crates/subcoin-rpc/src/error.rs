use bitcoin::consensus::encode::FromHexError;
use jsonrpsee::types::error::ErrorObject;
use jsonrpsee::types::ErrorObjectOwned;

/// Chain RPC Result type.
pub type Result<T> = std::result::Result<T, Error>;

/// Chain RPC errors.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("block not found")]
    BlockNotFound,
    #[error("substrate block hash not found")]
    SubstrateBlockHashNotFound,
    #[error("Invalid header: {0:?}")]
    Header(subcoin_primitives::HeaderError),
    #[error(transparent)]
    Blockchain(#[from] sp_blockchain::Error),
    #[error(transparent)]
    DecodeHex(#[from] FromHexError),
    #[error(transparent)]
    SerdeJson(#[from] serde_json::Error),
    /// Client error.
    #[error("Client error: {0}")]
    Client(#[from] Box<dyn std::error::Error + Send + Sync>),
    /// Other error type.
    #[error("{0}")]
    Other(String),
}

/// Base error code for RPC modules.
pub mod base {
    pub const BLOCKCHAIN: i32 = 10000;
}

/// Base error code for all chain errors.
const BASE_ERROR: i32 = base::BLOCKCHAIN;

impl From<Error> for ErrorObjectOwned {
    fn from(e: Error) -> ErrorObjectOwned {
        match e {
            Error::Other(message) => ErrorObject::owned(BASE_ERROR + 1, message, None::<()>),
            e => ErrorObject::owned(BASE_ERROR + 2, e.to_string(), None::<()>),
        }
    }
}
