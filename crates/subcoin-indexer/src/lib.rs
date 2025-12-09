//! SQLite-based blockchain indexer for Subcoin.
//!
//! This crate provides transaction and address indexing functionality using SQLite,
//! enabling efficient queries for:
//! - Transaction lookup by txid
//! - Address transaction history
//! - UTXO queries by address
//! - Address balance calculation

mod db;
mod indexer;
mod queries;
mod types;

pub use db::IndexerDatabase;
pub use indexer::Indexer;
pub use queries::IndexerQuery;
pub use types::{AddressBalance, AddressHistory, IndexerStatus, Utxo};
