//! SQLite database management for the indexer.

use crate::types::IndexerState;
use bitcoin::hashes::{Hash, hash160};
use bitcoin::key::CompressedPublicKey;
use bitcoin::{Address, Network, OutPoint, PubkeyHash, ScriptBuf, Txid};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use std::collections::{HashMap, HashSet};
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

/// Extract an address from a script_pubkey, including P2PK scripts.
///
/// This handles standard address types (P2PKH, P2SH, P2WPKH, P2WSH, P2TR) via
/// `Address::from_script`, plus P2PK (Pay-to-Public-Key) scripts which are
/// converted to their equivalent P2PKH addresses.
fn script_to_address(script: &ScriptBuf, network: Network) -> Option<String> {
    // Try standard address types first
    if let Ok(address) = Address::from_script(script.as_script(), network) {
        return Some(address.to_string());
    }

    // Handle P2PK (Pay-to-Public-Key) scripts
    // P2PK format: <pubkey_len> <pubkey> OP_CHECKSIG (0xac)
    let bytes = script.as_bytes();

    // Compressed P2PK: 0x21 (33) + 33-byte pubkey + 0xac (OP_CHECKSIG) = 35 bytes
    if bytes.len() == 35 && bytes[0] == 0x21 && bytes[34] == 0xac {
        let pubkey_bytes = &bytes[1..34];
        if let Ok(pubkey) = CompressedPublicKey::from_slice(pubkey_bytes) {
            let pubkey_hash = PubkeyHash::from(pubkey);
            let address = Address::p2pkh(pubkey_hash, network);
            return Some(address.to_string());
        }
    }

    // Uncompressed P2PK: 0x41 (65) + 65-byte pubkey + 0xac (OP_CHECKSIG) = 67 bytes
    if bytes.len() == 67 && bytes[0] == 0x41 && bytes[66] == 0xac {
        let pubkey_bytes = &bytes[1..66];
        let hash = hash160::Hash::hash(pubkey_bytes);
        let pubkey_hash = PubkeyHash::from_raw_hash(hash);
        let address = Address::p2pkh(pubkey_hash, network);
        return Some(address.to_string());
    }

    None
}

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
        let network_dir = network.to_core_arg();
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
        let address = script_to_address(script_pubkey, self.network);

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
    ///
    /// Optimized with:
    /// 1. Bulk pre-fetch of outputs (eliminates N+1 queries)
    /// 2. Collect all data in memory first, then bulk insert by table
    /// 3. Pre-aggregate address history deltas (avoids ON CONFLICT overhead)
    #[allow(clippy::type_complexity)]
    pub async fn index_blocks_batch(
        &self,
        blocks: &[(u32, bitcoin::Block)],
        network: Network,
    ) -> Result<()> {
        if blocks.is_empty() {
            return Ok(());
        }

        // ========== Phase 1: Collect all outpoints that need lookup ==========
        let mut outpoints_to_fetch: HashSet<OutPoint> = HashSet::new();
        for (_height, block) in blocks {
            for btc_tx in &block.txdata {
                for input in &btc_tx.input {
                    if !input.previous_output.is_null() {
                        outpoints_to_fetch.insert(input.previous_output);
                    }
                }
            }
        }

        // ========== Phase 2: Bulk fetch outputs ==========
        let mut output_cache: HashMap<OutPoint, (String, u64)> = HashMap::new();
        let outpoints_vec: Vec<_> = outpoints_to_fetch.into_iter().collect();

        let mut tx = self.pool.begin().await?;

        const FETCH_BATCH_SIZE: usize = 500;
        for chunk in outpoints_vec.chunks(FETCH_BATCH_SIZE) {
            if chunk.is_empty() {
                continue;
            }

            let placeholders: Vec<String> = chunk
                .iter()
                .map(|_| "(txid = ? AND vout = ?)".to_string())
                .collect();
            let query = format!(
                "SELECT txid, vout, address, value FROM outputs WHERE address IS NOT NULL AND ({})",
                placeholders.join(" OR ")
            );

            let mut query_builder = sqlx::query_as::<_, (Vec<u8>, i64, String, i64)>(&query);
            for outpoint in chunk {
                query_builder = query_builder
                    .bind(outpoint.txid.as_byte_array().as_slice())
                    .bind(outpoint.vout as i64);
            }

            let rows = query_builder.fetch_all(&mut *tx).await?;

            for (txid_bytes, vout, address, value) in rows {
                let txid = Self::parse_txid(&txid_bytes)?;
                let outpoint = OutPoint {
                    txid,
                    vout: vout as u32,
                };
                output_cache.insert(outpoint, (address, value as u64));
            }
        }

        // ========== Phase 3: Collect all data into memory buffers ==========

        // Block records: (height, hash, timestamp)
        let mut blocks_data: Vec<(i64, Vec<u8>, i64)> = Vec::new();

        // Transaction records: (txid, block_height, tx_index)
        let mut transactions_data: Vec<(Vec<u8>, i64, i64)> = Vec::new();

        // Output records: (txid, vout, address, value, script_pubkey, block_height)
        let mut outputs_data: Vec<(Vec<u8>, i64, Option<String>, i64, Vec<u8>, i64)> = Vec::new();

        // Spent output updates: (spent_txid, spent_vout, spent_block_height, orig_txid, orig_vout)
        let mut spent_updates: Vec<(Vec<u8>, i64, i64, Vec<u8>, i64)> = Vec::new();

        // Address history: HashMap<(address, txid_bytes, block_height), delta> - pre-aggregated
        let mut address_history_map: HashMap<(String, Vec<u8>, i64), i64> = HashMap::new();

        // Track outputs created within this batch (for intra-batch spending)
        // Key: OutPoint, Value: (address, value)
        let mut batch_outputs: HashMap<OutPoint, (Option<String>, u64)> = HashMap::new();

        for (height, block) in blocks {
            let block_hash = block.block_hash();
            let timestamp = block.header.time;

            blocks_data.push((
                *height as i64,
                block_hash.as_byte_array().to_vec(),
                timestamp as i64,
            ));

            for (tx_index, btc_tx) in block.txdata.iter().enumerate() {
                let txid = btc_tx.compute_txid();
                let txid_bytes = txid.as_byte_array().to_vec();

                transactions_data.push((txid_bytes.clone(), *height as i64, tx_index as i64));

                // Process inputs - check both database cache and batch-local outputs
                for (vin, input) in btc_tx.input.iter().enumerate() {
                    if input.previous_output.is_null() {
                        continue;
                    }

                    // Try database cache first, then batch-local outputs
                    let output_info: Option<(String, u64)> = output_cache
                        .get(&input.previous_output)
                        .cloned()
                        .or_else(|| {
                            batch_outputs
                                .get(&input.previous_output)
                                .and_then(|(addr, val)| addr.clone().map(|a| (a, *val)))
                        });

                    if let Some((address, value)) = output_info {
                        spent_updates.push((
                            txid_bytes.clone(),
                            vin as i64,
                            *height as i64,
                            input.previous_output.txid.as_byte_array().to_vec(),
                            input.previous_output.vout as i64,
                        ));

                        // Accumulate negative delta
                        let key = (address.clone(), txid_bytes.clone(), *height as i64);
                        *address_history_map.entry(key).or_insert(0) -= value as i64;
                    }
                }

                // Process outputs
                for (vout, output) in btc_tx.output.iter().enumerate() {
                    let script = &output.script_pubkey;
                    let value = output.value.to_sat();

                    let address = script_to_address(script, network);

                    outputs_data.push((
                        txid_bytes.clone(),
                        vout as i64,
                        address.clone(),
                        value as i64,
                        script.as_bytes().to_vec(),
                        *height as i64,
                    ));

                    // Track this output for potential intra-batch spending
                    let outpoint = OutPoint {
                        txid,
                        vout: vout as u32,
                    };
                    batch_outputs.insert(outpoint, (address.clone(), value));

                    // Accumulate positive delta
                    if let Some(addr) = address {
                        let key = (addr, txid_bytes.clone(), *height as i64);
                        *address_history_map.entry(key).or_insert(0) += value as i64;
                    }
                }
            }
        }

        // ========== Phase 4: Bulk insert by table ==========

        // 4a: Insert blocks (multi-row INSERT)
        const INSERT_BATCH_SIZE: usize = 100;

        for chunk in blocks_data.chunks(INSERT_BATCH_SIZE) {
            let placeholders: Vec<&str> = chunk.iter().map(|_| "(?, ?, ?)").collect();
            let query = format!(
                "INSERT OR REPLACE INTO blocks (height, hash, timestamp) VALUES {}",
                placeholders.join(", ")
            );

            let mut query_builder = sqlx::query(&query);
            for (height, hash, timestamp) in chunk {
                query_builder = query_builder.bind(height).bind(hash).bind(timestamp);
            }
            query_builder.execute(&mut *tx).await?;
        }

        // 4b: Insert transactions (multi-row INSERT)
        for chunk in transactions_data.chunks(INSERT_BATCH_SIZE) {
            let placeholders: Vec<&str> = chunk.iter().map(|_| "(?, ?, ?)").collect();
            let query = format!(
                "INSERT OR REPLACE INTO transactions (txid, block_height, tx_index) VALUES {}",
                placeholders.join(", ")
            );

            let mut query_builder = sqlx::query(&query);
            for (txid, height, idx) in chunk {
                query_builder = query_builder.bind(txid).bind(height).bind(idx);
            }
            query_builder.execute(&mut *tx).await?;
        }

        // 4c: Insert outputs (multi-row INSERT)
        for chunk in outputs_data.chunks(INSERT_BATCH_SIZE) {
            let placeholders: Vec<&str> = chunk.iter().map(|_| "(?, ?, ?, ?, ?, ?)").collect();
            let query = format!(
                "INSERT OR REPLACE INTO outputs (txid, vout, address, value, script_pubkey, block_height) VALUES {}",
                placeholders.join(", ")
            );

            let mut query_builder = sqlx::query(&query);
            for (txid, vout, address, value, script, height) in chunk {
                query_builder = query_builder
                    .bind(txid)
                    .bind(vout)
                    .bind(address)
                    .bind(value)
                    .bind(script)
                    .bind(height);
            }
            query_builder.execute(&mut *tx).await?;
        }

        // 4d: Update spent outputs - individual UPDATEs using PK index
        // Direct PK lookups are fast; the temp table approach with correlated subqueries
        // becomes slow as the outputs table grows
        for (spent_txid, spent_vout, spent_height, orig_txid, orig_vout) in &spent_updates {
            sqlx::query(
                "UPDATE outputs SET spent_txid = ?, spent_vout = ?, spent_block_height = ? WHERE txid = ? AND vout = ?",
            )
            .bind(spent_txid)
            .bind(spent_vout)
            .bind(spent_height)
            .bind(orig_txid)
            .bind(orig_vout)
            .execute(&mut *tx)
            .await?;
        }

        // 4e: Insert address history (multi-row INSERT with pre-aggregated deltas)
        let history_entries: Vec<_> = address_history_map.into_iter().collect();
        for chunk in history_entries.chunks(INSERT_BATCH_SIZE) {
            let placeholders: Vec<&str> = chunk.iter().map(|_| "(?, ?, ?, ?)").collect();
            let query = format!(
                "INSERT OR REPLACE INTO address_history (address, txid, block_height, delta) VALUES {}",
                placeholders.join(", ")
            );

            let mut query_builder = sqlx::query(&query);
            for ((address, txid, height), delta) in chunk {
                query_builder = query_builder
                    .bind(address)
                    .bind(txid)
                    .bind(height)
                    .bind(delta);
            }
            query_builder.execute(&mut *tx).await?;
        }

        tx.commit().await?;
        Ok(())
    }
}
