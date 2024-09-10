mod block_executor;
mod block_import;
mod chain_params;
mod import_queue;
mod metrics;
mod verification;

pub use block_executor::{
    BenchmarkAllExecutor, BenchmarkRuntimeBlockExecutor, BlockExecutionStrategy, BlockExecutor,
    ClientContext, ExecutionBackend, OffRuntimeBlockExecutor, RuntimeBlockExecutor,
};
pub use block_import::{
    insert_bitcoin_block_hash_mapping, BitcoinBlockImport, BitcoinBlockImporter, ImportConfig,
    ImportStatus,
};
pub use chain_params::ChainParams;
pub use import_queue::{
    bitcoin_import_queue, BlockImportQueue, ImportBlocks, ImportManyBlocksResult,
};
pub use verification::{BlockVerification, BlockVerifier, HeaderError, HeaderVerifier};

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
