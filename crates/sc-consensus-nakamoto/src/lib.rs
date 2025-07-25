mod aux_schema;
mod block_import;
mod chain_params;
mod import_queue;
mod metrics;
mod verification;
mod verifier;

pub use block_import::{BitcoinBlockImport, BitcoinBlockImporter, ImportConfig, ImportStatus};
pub use chain_params::ChainParams;
pub use import_queue::{
    BlockImportQueue, ImportBlocks, ImportManyBlocksResult, bitcoin_import_queue,
};
pub use verification::{
    BlockVerification, BlockVerifier, HeaderError, HeaderVerifier, ScriptEngine,
};
pub use verifier::SubstrateImportQueueVerifier;

/// Consensus error type.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Header uses the wrong engine {0:?}")]
    WrongEngine([u8; 4]),
    #[error("Multiple pre-runtime digests")]
    MultiplePreRuntimeDigests,
    #[error("bitcoin block hash not found in the header diegst")]
    MissingBitcoinBlockHashDigest,
    #[error("invalid bitcoin block hash in the header diegst")]
    InvalidBitcoinBlockHashDigest,
    #[error(transparent)]
    Client(sp_blockchain::Error),
    #[error(transparent)]
    Codec(codec::Error),
}
