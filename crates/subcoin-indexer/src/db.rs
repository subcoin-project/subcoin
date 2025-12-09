//! SQLite database management for the indexer.

use crate::types::IndexerState;
use bitcoin::hashes::Hash;
use bitcoin::{Address, Network, OutPoint, ScriptBuf, Txid};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use std::path::Path;

/// Indexer error type.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Invalid txid in database: {0}")]
    InvalidTxid(String),

    #[error("Block not found: {0}")]
    BlockNotFound(u32),
}

pub type Result<T> = std::result::Result<T, Error>;

/// SQLite database for the indexer.
#[derive(Clone)]
pub struct IndexerDatabase {
    pool: SqlitePool,
    network: Network,
}

impl IndexerDatabase {
    /// Opens or creates the indexer database at the given path.
    ///
    /// The database file is stored in a network-specific subdirectory to prevent
    /// mixing data from different networks (mainnet, testnet, signet, etc.).
    pub async fn open(path: &Path, network: Network) -> Result<Self> {
        let network_dir = match network {
            Network::Bitcoin => "mainnet",
            Network::Testnet => "testnet",
            Network::Signet => "signet",
            Network::Regtest => "regtest",
            _ => "unknown",
        };
        let db_path = path.join("indexer").join(network_dir).join("index.sqlite");

        // Ensure parent directory exists
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        let options = SqliteConnectOptions::new()
            .filename(&db_path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .pragma("cache_size", "-64000") // 64MB cache
            .pragma("synchronous", "NORMAL");

        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await?;

        let db = Self { pool, network };
        db.init_schema().await?;

        Ok(db)
    }

    /// Initialize database schema.
    async fn init_schema(&self) -> Result<()> {
        sqlx::query(
            r#"
            -- Blocks table for reorg tracking
            CREATE TABLE IF NOT EXISTS blocks (
                height INTEGER PRIMARY KEY,
                hash BLOB NOT NULL UNIQUE,
                timestamp INTEGER NOT NULL
            );

            -- Transactions table
            CREATE TABLE IF NOT EXISTS transactions (
                txid BLOB PRIMARY KEY,
                block_height INTEGER NOT NULL,
                tx_index INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_tx_block ON transactions(block_height);

            -- Outputs table (for UTXO tracking)
            CREATE TABLE IF NOT EXISTS outputs (
                txid BLOB NOT NULL,
                vout INTEGER NOT NULL,
                address TEXT,
                value INTEGER NOT NULL,
                script_pubkey BLOB NOT NULL,
                block_height INTEGER NOT NULL,
                spent_txid BLOB,
                spent_vout INTEGER,
                spent_block_height INTEGER,
                PRIMARY KEY (txid, vout)
            );
            CREATE INDEX IF NOT EXISTS idx_outputs_address ON outputs(address) WHERE address IS NOT NULL;
            CREATE INDEX IF NOT EXISTS idx_outputs_unspent ON outputs(address) WHERE address IS NOT NULL AND spent_txid IS NULL;
            CREATE INDEX IF NOT EXISTS idx_outputs_block ON outputs(block_height);

            -- Address history (denormalized for fast queries)
            CREATE TABLE IF NOT EXISTS address_history (
                address TEXT NOT NULL,
                txid BLOB NOT NULL,
                block_height INTEGER NOT NULL,
                delta INTEGER NOT NULL,
                PRIMARY KEY (address, block_height, txid)
            );
            CREATE INDEX IF NOT EXISTS idx_addr_hist ON address_history(address, block_height DESC);

            -- Indexer state
            CREATE TABLE IF NOT EXISTS state (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Get the underlying connection pool.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Get the Bitcoin network.
    pub fn network(&self) -> Network {
        self.network
    }

    // ========== State Management ==========

    /// Load indexer state from database.
    pub async fn load_state(&self) -> Result<Option<IndexerState>> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT value FROM state WHERE key = 'indexer_state'")
                .fetch_optional(&self.pool)
                .await?;

        let Some((value,)) = row else {
            return Ok(None);
        };

        // Parse state from JSON-like format
        if value.starts_with("historical:") {
            let parts: Vec<&str> = value
                .strip_prefix("historical:")
                .unwrap()
                .split(':')
                .collect();
            if parts.len() == 2 {
                let target_height: u32 = parts[0].parse().unwrap_or(0);
                let current_height: u32 = parts[1].parse().unwrap_or(0);
                return Ok(Some(IndexerState::HistoricalIndexing {
                    target_height,
                    current_height,
                }));
            }
        } else if value.starts_with("active:") {
            let last_indexed: u32 = value.strip_prefix("active:").unwrap().parse().unwrap_or(0);
            return Ok(Some(IndexerState::Active { last_indexed }));
        }

        Ok(None)
    }

    /// Save indexer state to database.
    pub async fn save_state(&self, state: &IndexerState) -> Result<()> {
        let value = match state {
            IndexerState::HistoricalIndexing {
                target_height,
                current_height,
            } => format!("historical:{target_height}:{current_height}"),
            IndexerState::Active { last_indexed } => format!("active:{last_indexed}"),
        };

        sqlx::query("INSERT OR REPLACE INTO state (key, value) VALUES ('indexer_state', ?)")
            .bind(&value)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    // ========== Block Operations ==========

    /// Insert a block record.
    pub async fn insert_block(&self, height: u32, hash: &[u8; 32], timestamp: u32) -> Result<()> {
        sqlx::query("INSERT OR REPLACE INTO blocks (height, hash, timestamp) VALUES (?, ?, ?)")
            .bind(height as i64)
            .bind(hash.as_slice())
            .bind(timestamp as i64)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Get the last indexed block height.
    pub async fn last_block_height(&self) -> Result<Option<u32>> {
        let row: Option<(i64,)> = sqlx::query_as("SELECT MAX(height) FROM blocks")
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.and_then(|(h,)| if h >= 0 { Some(h as u32) } else { None }))
    }

    // ========== Transaction Operations ==========

    /// Insert a transaction record.
    pub async fn insert_transaction(
        &self,
        txid: &Txid,
        block_height: u32,
        tx_index: u32,
    ) -> Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO transactions (txid, block_height, tx_index) VALUES (?, ?, ?)",
        )
        .bind(txid.as_byte_array().as_slice())
        .bind(block_height as i64)
        .bind(tx_index as i64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Get transaction position by txid.
    pub async fn get_transaction(&self, txid: &Txid) -> Result<Option<(u32, u32)>> {
        let row: Option<(i64, i64)> =
            sqlx::query_as("SELECT block_height, tx_index FROM transactions WHERE txid = ?")
                .bind(txid.as_byte_array().as_slice())
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|(h, i)| (h as u32, i as u32)))
    }

    // ========== Output Operations ==========

    /// Insert an output record.
    pub async fn insert_output(
        &self,
        txid: &Txid,
        vout: u32,
        script_pubkey: &ScriptBuf,
        value: u64,
        block_height: u32,
    ) -> Result<()> {
        let address = Address::from_script(script_pubkey.as_script(), self.network)
            .ok()
            .map(|a| a.to_string());

        sqlx::query(
            "INSERT OR REPLACE INTO outputs (txid, vout, address, value, script_pubkey, block_height) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(txid.as_byte_array().as_slice())
        .bind(vout as i64)
        .bind(&address)
        .bind(value as i64)
        .bind(script_pubkey.as_bytes())
        .bind(block_height as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Mark an output as spent.
    pub async fn mark_output_spent(
        &self,
        outpoint: &OutPoint,
        spent_txid: &Txid,
        spent_vout: u32,
        spent_block_height: u32,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE outputs SET spent_txid = ?, spent_vout = ?, spent_block_height = ? WHERE txid = ? AND vout = ?",
        )
        .bind(spent_txid.as_byte_array().as_slice())
        .bind(spent_vout as i64)
        .bind(spent_block_height as i64)
        .bind(outpoint.txid.as_byte_array().as_slice())
        .bind(outpoint.vout as i64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Get the address for an output (for computing deltas during spending).
    pub async fn get_output_info(&self, outpoint: &OutPoint) -> Result<Option<(String, u64)>> {
        let row: Option<(String, i64)> =
            sqlx::query_as("SELECT address, value FROM outputs WHERE txid = ? AND vout = ? AND address IS NOT NULL")
                .bind(outpoint.txid.as_byte_array().as_slice())
                .bind(outpoint.vout as i64)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|(addr, val)| (addr, val as u64)))
    }

    // ========== Address History Operations ==========

    /// Insert an address history entry.
    pub async fn insert_address_history(
        &self,
        address: &str,
        txid: &Txid,
        block_height: u32,
        delta: i64,
    ) -> Result<()> {
        // Use INSERT OR REPLACE to handle the case where an address appears in both
        // inputs and outputs of the same transaction - we want to sum the deltas
        sqlx::query(
            r#"
            INSERT INTO address_history (address, txid, block_height, delta)
            VALUES (?, ?, ?, ?)
            ON CONFLICT(address, block_height, txid) DO UPDATE SET
                delta = address_history.delta + excluded.delta
            "#,
        )
        .bind(address)
        .bind(txid.as_byte_array().as_slice())
        .bind(block_height as i64)
        .bind(delta)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // ========== Reorg Handling ==========

    /// Revert all data above the given block height (for handling reorgs).
    pub async fn revert_to_height(&self, height: u32) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        // Unspend outputs that were spent in reverted blocks
        sqlx::query(
            "UPDATE outputs SET spent_txid = NULL, spent_vout = NULL, spent_block_height = NULL WHERE spent_block_height > ?",
        )
        .bind(height as i64)
        .execute(&mut *tx)
        .await?;

        // Delete outputs created in reverted blocks
        sqlx::query("DELETE FROM outputs WHERE block_height > ?")
            .bind(height as i64)
            .execute(&mut *tx)
            .await?;

        // Delete transactions in reverted blocks
        sqlx::query("DELETE FROM transactions WHERE block_height > ?")
            .bind(height as i64)
            .execute(&mut *tx)
            .await?;

        // Delete address history in reverted blocks
        sqlx::query("DELETE FROM address_history WHERE block_height > ?")
            .bind(height as i64)
            .execute(&mut *tx)
            .await?;

        // Delete blocks
        sqlx::query("DELETE FROM blocks WHERE height > ?")
            .bind(height as i64)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(())
    }

    /// Begin a database transaction for batch operations.
    pub async fn begin_transaction(&self) -> Result<sqlx::Transaction<'_, sqlx::Sqlite>> {
        Ok(self.pool.begin().await?)
    }

    /// Parse a txid from database bytes.
    pub fn parse_txid(bytes: &[u8]) -> Result<Txid> {
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| Error::InvalidTxid("Invalid length".to_string()))?;
        Ok(Txid::from_byte_array(arr))
    }

    /// Index a single block in a database transaction (for live sync).
    pub async fn index_block(
        &self,
        height: u32,
        block: &bitcoin::Block,
        network: Network,
    ) -> Result<()> {
        self.index_blocks_batch(&[(height, block.clone())], network)
            .await
    }

    /// Index multiple blocks in a single database transaction for better performance.
    /// Used during historical sync for batching, and indirectly for single blocks during live sync.
    pub async fn index_blocks_batch(
        &self,
        blocks: &[(u32, bitcoin::Block)],
        network: Network,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        for (height, block) in blocks {
            let block_hash = block.block_hash();
            let timestamp = block.header.time;

            // Insert block
            sqlx::query("INSERT OR REPLACE INTO blocks (height, hash, timestamp) VALUES (?, ?, ?)")
                .bind(*height as i64)
                .bind(block_hash.as_byte_array().as_slice())
                .bind(timestamp as i64)
                .execute(&mut *tx)
                .await?;

            // Process each transaction
            for (tx_index, btc_tx) in block.txdata.iter().enumerate() {
                let txid = btc_tx.compute_txid();

                // Insert transaction record
                sqlx::query(
                    "INSERT OR REPLACE INTO transactions (txid, block_height, tx_index) VALUES (?, ?, ?)",
                )
                .bind(txid.as_byte_array().as_slice())
                .bind(*height as i64)
                .bind(tx_index as i64)
                .execute(&mut *tx)
                .await?;

                // Process inputs (mark outputs as spent, record address history)
                for (vin, input) in btc_tx.input.iter().enumerate() {
                    if input.previous_output.is_null() {
                        // Coinbase input, skip
                        continue;
                    }

                    // Get the spent output's address and value
                    let row: Option<(String, i64)> = sqlx::query_as(
                        "SELECT address, value FROM outputs WHERE txid = ? AND vout = ? AND address IS NOT NULL",
                    )
                    .bind(input.previous_output.txid.as_byte_array().as_slice())
                    .bind(input.previous_output.vout as i64)
                    .fetch_optional(&mut *tx)
                    .await?;

                    if let Some((address, value)) = row {
                        // Mark the output as spent
                        sqlx::query(
                            "UPDATE outputs SET spent_txid = ?, spent_vout = ?, spent_block_height = ? WHERE txid = ? AND vout = ?",
                        )
                        .bind(txid.as_byte_array().as_slice())
                        .bind(vin as i64)
                        .bind(*height as i64)
                        .bind(input.previous_output.txid.as_byte_array().as_slice())
                        .bind(input.previous_output.vout as i64)
                        .execute(&mut *tx)
                        .await?;

                        // Record negative delta in address history (spending)
                        sqlx::query(
                            r#"
                            INSERT INTO address_history (address, txid, block_height, delta)
                            VALUES (?, ?, ?, ?)
                            ON CONFLICT(address, block_height, txid) DO UPDATE SET
                                delta = address_history.delta + excluded.delta
                            "#,
                        )
                        .bind(&address)
                        .bind(txid.as_byte_array().as_slice())
                        .bind(*height as i64)
                        .bind(-value)
                        .execute(&mut *tx)
                        .await?;
                    }
                }

                // Process outputs (create UTXOs, record address history)
                for (vout, output) in btc_tx.output.iter().enumerate() {
                    let script = &output.script_pubkey;
                    let value = output.value.to_sat();

                    let address = Address::from_script(script.as_script(), network)
                        .ok()
                        .map(|a| a.to_string());

                    // Insert output
                    sqlx::query(
                        "INSERT OR REPLACE INTO outputs (txid, vout, address, value, script_pubkey, block_height) VALUES (?, ?, ?, ?, ?, ?)",
                    )
                    .bind(txid.as_byte_array().as_slice())
                    .bind(vout as i64)
                    .bind(&address)
                    .bind(value as i64)
                    .bind(script.as_bytes())
                    .bind(*height as i64)
                    .execute(&mut *tx)
                    .await?;

                    // If we can derive an address, record history
                    if let Some(ref addr) = address {
                        sqlx::query(
                            r#"
                            INSERT INTO address_history (address, txid, block_height, delta)
                            VALUES (?, ?, ?, ?)
                            ON CONFLICT(address, block_height, txid) DO UPDATE SET
                                delta = address_history.delta + excluded.delta
                            "#,
                        )
                        .bind(addr)
                        .bind(txid.as_byte_array().as_slice())
                        .bind(*height as i64)
                        .bind(value as i64)
                        .execute(&mut *tx)
                        .await?;
                    }
                }
            }
        }

        tx.commit().await?;
        Ok(())
    }
}
