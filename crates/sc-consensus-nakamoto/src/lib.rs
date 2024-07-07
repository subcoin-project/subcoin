mod block_executor;
mod block_import;
mod verification;

pub use block_import::{BitcoinBlockImport, BitcoinBlockImporter, ImportConfig, ImportStatus};
pub use verification::BlockVerification;

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