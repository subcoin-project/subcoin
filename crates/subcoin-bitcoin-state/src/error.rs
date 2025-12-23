//! Error types for native UTXO storage.

use bitcoin::OutPoint;

/// Errors that can occur during UTXO storage operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// RocksDB error.
    #[error("RocksDB error: {0}")]
    Rocksdb(#[from] rocksdb::Error),

    /// Serialization error.
    #[error("Serialization error: {0}")]
    Serialization(String),

    /// Deserialization error.
    #[error("Deserialization error: {0}")]
    Deserialization(String),

    /// UTXO not found when trying to spend.
    #[error("UTXO not found: {0}")]
    UtxoNotFound(OutPoint),

    /// Duplicate UTXO (already exists).
    #[error("Duplicate UTXO: {0}")]
    DuplicateUtxo(OutPoint),

    /// Block undo data not found.
    #[error("Undo data not found for height {0}")]
    UndoNotFound(u32),

    /// MuHash mismatch at checkpoint.
    #[error("MuHash mismatch at height {height}: expected {expected}, got {actual}")]
    MuHashMismatch {
        height: u32,
        expected: String,
        actual: String,
    },

    /// Invalid height (e.g., trying to revert below genesis).
    #[error("Invalid height: {0}")]
    InvalidHeight(String),

    /// Storage not initialized.
    #[error("Storage not initialized")]
    NotInitialized,

    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

impl From<bincode::Error> for Error {
    fn from(err: bincode::Error) -> Self {
        Self::Serialization(err.to_string())
    }
}
