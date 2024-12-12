use bitcoin::consensus::encode::FromHexError;
use jsonrpsee::types::error::ErrorObject;
use jsonrpsee::types::ErrorObjectOwned;
use sc_rpc_api::UnsafeRpcError;

/// Chain RPC errors.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("block not found")]
    BlockNotFound,
    #[error("Bitcoin P2P network service unavailable")]
    NetworkUnavailable,
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
    /// Call to an unsafe RPC was denied.
    #[error(transparent)]
    UnsafeRpcCalled(#[from] UnsafeRpcError),
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
