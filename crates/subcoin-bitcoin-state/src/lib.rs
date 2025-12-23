//! Bitcoin state (UTXO set) storage with MuHash commitment for Subcoin.
//!
//! This crate provides a high-performance Bitcoin state implementation that bypasses
//! Substrate's Merkle Patricia Trie, using direct RocksDB storage with MuHash commitment.
//!
//! ## Architecture
//!
//! - **UTXO Storage**: Direct RocksDB key-value storage for O(1) lookups
//! - **MuHash Commitment**: Rolling hash updated O(1) per UTXO change
//! - **Undo Data**: Per-block undo information for chain reorganizations
//!
//! ## Performance
//!
//! Unlike Substrate's trie-based storage which has O(log n) per-operation overhead,
//! this implementation provides:
//! - O(1) UTXO insert/delete
//! - O(1) MuHash update per UTXO
//! - Constant-time commitment computation

mod coin;
mod error;
mod storage;
mod undo;

pub use coin::Coin;
pub use error::Error;
pub use storage::{BitcoinState, UtxoIterator};
pub use undo::BlockUndo;

/// Result type for UTXO storage operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Result type for export_chunk operation.
/// Contains: (entries, next_cursor, is_complete)
pub type ExportChunkResult = (Vec<(bitcoin::OutPoint, Coin)>, Option<[u8; 36]>, bool);

/// Column family names for RocksDB.
mod cf {
    /// Column family for UTXO entries.
    /// Key: OutPoint (txid || vout) = 36 bytes
    /// Value: Coin (serialized)
    pub const UTXOS: &str = "utxos";

    /// Column family for block undo data.
    /// Key: block height (u32, big-endian)
    /// Value: BlockUndo (serialized)
    pub const UNDO: &str = "undo";

    /// Column family for metadata.
    /// Keys: "height", "utxo_count", "muhash"
    pub const META: &str = "meta";
}

/// Metadata keys.
mod meta_keys {
    pub const HEIGHT: &[u8] = b"height";
    pub const UTXO_COUNT: &[u8] = b"utxo_count";
    pub const MUHASH: &[u8] = b"muhash";
}
