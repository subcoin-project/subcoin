//! Types for the indexer.

use bitcoin::Txid;

/// A transaction in an address's history.
#[derive(Debug, Clone)]
pub struct AddressHistory {
    /// Transaction ID.
    pub txid: Txid,
    /// Block height of the transaction.
    pub block_height: u32,
    /// Net change in satoshis for this address in this transaction.
    /// Positive = received, negative = sent.
    pub delta: i64,
    /// Block timestamp.
    pub timestamp: u32,
}

/// An unspent transaction output.
#[derive(Debug, Clone)]
pub struct Utxo {
    /// Transaction ID containing this output.
    pub txid: Txid,
    /// Output index within the transaction.
    pub vout: u32,
    /// Value in satoshis.
    pub value: u64,
    /// Block height where this output was created.
    pub block_height: u32,
    /// The scriptPubKey as hex.
    pub script_pubkey: Vec<u8>,
}

/// Address balance summary.
#[derive(Debug, Clone, Default)]
pub struct AddressBalance {
    /// Confirmed balance in satoshis.
    pub confirmed: u64,
    /// Number of transactions involving this address.
    pub tx_count: u64,
    /// Number of unspent outputs.
    pub utxo_count: u64,
    /// Total received in satoshis.
    pub total_received: u64,
    /// Total sent in satoshis.
    pub total_sent: u64,
}

/// Indexer state for crash recovery.
#[derive(Debug, Clone, Copy)]
pub enum IndexerState {
    /// Currently indexing historical blocks.
    HistoricalIndexing {
        /// Target block height to reach.
        target_height: u32,
        /// Current position in historical indexing.
        current_height: u32,
    },
    /// Actively processing new blocks.
    Active {
        /// Last successfully indexed block height.
        last_indexed: u32,
    },
}

/// Indexer status for RPC queries.
#[derive(Debug, Clone)]
pub struct IndexerStatus {
    /// Whether the indexer is currently syncing historical blocks.
    pub is_syncing: bool,
    /// Current indexed block height.
    pub indexed_height: u32,
    /// Target block height (only relevant during historical sync).
    pub target_height: Option<u32>,
    /// Sync progress as percentage (0.0 - 100.0).
    pub progress_percent: f64,
}

/// Address statistics for RPC queries.
#[derive(Debug, Clone, Default)]
pub struct AddressStats {
    /// Block height when address first received funds.
    pub first_seen_height: Option<u32>,
    /// Timestamp when address first received funds.
    pub first_seen_timestamp: Option<u32>,
    /// Block height of most recent transaction.
    pub last_seen_height: Option<u32>,
    /// Timestamp of most recent transaction.
    pub last_seen_timestamp: Option<u32>,
    /// Largest single receive amount in satoshis.
    pub largest_receive: u64,
    /// Largest single send amount in satoshis (absolute value).
    pub largest_send: u64,
    /// Total number of receive transactions.
    pub receive_count: u64,
    /// Total number of send transactions.
    pub send_count: u64,
}

/// Output spending status.
#[derive(Debug, Clone)]
pub struct OutputStatus {
    /// Whether the output has been spent.
    pub spent: bool,
    /// Transaction ID that spent this output (if spent).
    pub spent_by_txid: Option<Txid>,
    /// Input index in the spending transaction (if spent).
    pub spent_by_vin: Option<u32>,
    /// Block height where the output was spent (if spent).
    pub spent_at_height: Option<u32>,
}
