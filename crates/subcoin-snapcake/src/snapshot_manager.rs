use bitcoin::hashes::Hash;
use bitcoin::{BlockHash, Txid};
use rocksdb::DB;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, Instant};
use subcoin_runtime_primitives::Coin;
use subcoin_utxo_snapshot::{OutputEntry, Utxo, UtxoSnapshotGenerator};

const COUNT_KEY: &[u8; 12] = b"__coin_count";

const INTERVAL: Duration = Duration::from_secs(5);

const CURSOR_FILE: &str = "state_sync_cursor.bin";

/// Cursor for resumable state sync.
///
/// This struct stores the progress of a state sync operation, allowing it to be
/// resumed from where it left off after an interruption.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateSyncCursor {
    /// Target block number for the sync.
    pub target_block_number: u32,
    /// Target Bitcoin block hash for the sync.
    pub target_bitcoin_block_hash: BlockHash,
    /// The last key(s) processed, used to resume sync from this position.
    pub last_key: Vec<Vec<u8>>,
    /// Total number of coins expected.
    pub total_coins: u64,
    /// Number of coins received so far.
    pub received_coins: u64,
}

static MAINNET_SNAPSHOT_SHA256SUMS: LazyLock<HashMap<BlockHash, &str>> = LazyLock::new(|| {
    HashMap::from([(
        "0000000000000000000320283a032748cef8227873ff4872689bf23f1cda83a5"
            .parse()
            .expect("BlockHash for mainnet block#8400000 is correct; qed"),
        "dc4bb43d58d6a25e91eae93eb052d72e3318bd98ec62a5d0c11817cefbba177b",
    )])
});

/// Storage backend for UTXO snapshots.
///
/// Note that [`SnapshotStore::InMem`] and [`SnapshotStore::Csv`] can not be used in production as
/// they may consume huge RAM up to 30+ GB.
enum SnapshotStore {
    // Initially created for local testing.
    #[allow(unused)]
    InMem(Vec<Utxo>),
    /// Human readable format.
    ///
    /// This format worked before Bitcoin Core 28. The snapshot format has been changed since
    /// Bitcoin Core 28, but we still keep this format for the human readable property.
    #[allow(unused)]
    Csv(PathBuf),
    // Store the coins in lexicographical order.
    Rocksdb(DB),
}

impl SnapshotStore {
    /// Append a batch of UTXOs to the store.
    fn push_batch(&mut self, utxos: &[Utxo]) -> std::io::Result<usize> {
        match self {
            Self::InMem(list) => {
                let count = utxos.len();
                list.extend(utxos.iter().cloned());
                Ok(count)
            }
            Self::Csv(path) => {
                for utxo in utxos {
                    Self::append_to_csv(path, utxo.clone())?;
                }
                Ok(utxos.len())
            }
            Self::Rocksdb(db) => Self::insert_batch_to_rocksdb(db, utxos)
                .map_err(|err| std::io::Error::other(format!("rocksdb: {err:?}"))),
        }
    }

    fn append_to_csv(path: &Path, utxo: Utxo) -> std::io::Result<()> {
        // Open the file in append mode each time to avoid keeping it open across calls
        let file = std::fs::OpenOptions::new().append(true).open(path)?;

        let mut wtr = csv::WriterBuilder::new()
            .has_headers(false) // Disable automatic header writing
            .from_writer(file);

        let utxo_csv_entry = UtxoCsvEntry::from(utxo);
        if let Err(e) = wtr.serialize(&utxo_csv_entry) {
            return Err(std::io::Error::other(format!(
                "Failed to write UTXO entry to CSV: {e}"
            )));
        }

        wtr.flush()?;

        Ok(())
    }

    fn insert_batch_to_rocksdb(db: &DB, utxos: &[Utxo]) -> Result<usize, rocksdb::Error> {
        if utxos.is_empty() {
            return Ok(0);
        }

        let mut coin_count =
            Self::read_rocksdb_count(db).expect("Failed to read count from Rocksdb") as u64;

        let mut batch = rocksdb::WriteBatch::default();
        let batch_size = utxos.len();

        for utxo in utxos {
            let Utxo { txid, vout, coin } = utxo;

            let mut key = Vec::with_capacity(32 + 4);
            key.extend(txid.to_byte_array());
            key.extend(vout.to_be_bytes());

            let value = bincode::serialize(coin).expect("Failed to serialize Coin");
            batch.put(&key, value);
        }

        coin_count += batch_size as u64;
        batch.put(COUNT_KEY, coin_count.to_le_bytes());

        db.write(batch)?;

        Ok(batch_size)
    }

    /// Count total UTXO entries in the store.
    fn count(&self) -> std::io::Result<usize> {
        match self {
            Self::InMem(list) => Ok(list.len()),
            Self::Csv(path) => {
                // Open the file in read mode only for counting
                let file = std::fs::File::open(path)?;
                let reader = std::io::BufReader::new(file);

                // Count lines by reading through each record
                let count = csv::ReaderBuilder::new()
                    .has_headers(false)
                    .from_reader(reader)
                    .records()
                    .count();

                Ok(count)
            }
            Self::Rocksdb(db) => Self::read_rocksdb_count(db),
        }
    }

    /// Read the RocksDB UTXO count.
    fn read_rocksdb_count(db: &DB) -> std::io::Result<usize> {
        Ok(db
            .get(COUNT_KEY)
            .ok()
            .flatten()
            .map(|v| u64::from_le_bytes(v.try_into().expect("Invalid DB value")))
            .unwrap_or(0) as usize)
    }

    fn generate_snapshot(
        &mut self,
        snapshot_generator: &mut UtxoSnapshotGenerator,
        target_bitcoin_block_hash: BlockHash,
        utxos_count: u64,
    ) -> std::io::Result<()> {
        match self {
            Self::InMem(list) => snapshot_generator.generate_snapshot_in_mem(
                target_bitcoin_block_hash,
                utxos_count,
                std::mem::take(list),
            ),
            Self::Csv(path) => Self::generate_from_csv(
                path,
                snapshot_generator,
                target_bitcoin_block_hash,
                utxos_count,
            ),
            Self::Rocksdb(db) => Self::generate_from_rocksdb(
                db,
                snapshot_generator,
                target_bitcoin_block_hash,
                utxos_count,
            ),
        }
    }

    fn generate_from_csv(
        path: impl AsRef<Path>,
        snapshot_generator: &mut UtxoSnapshotGenerator,
        block_hash: BlockHash,
        utxos_count: u64,
    ) -> std::io::Result<()> {
        let file = std::fs::File::open(path)?;

        let reader = std::io::BufReader::new(file);
        let csv_reader = csv::ReaderBuilder::new()
            .has_headers(false)
            .from_reader(reader);

        let utxo_iter = csv_reader
            .into_deserialize::<UtxoCsvEntry>()
            .filter_map(Result::ok)
            .map(Utxo::from);

        snapshot_generator.generate_snapshot_in_mem(block_hash, utxos_count, utxo_iter)
    }

    fn generate_from_rocksdb(
        db: &DB,
        snapshot_generator: &mut UtxoSnapshotGenerator,
        bitcoin_block_hash: BlockHash,
        utxos_count: u64,
    ) -> std::io::Result<()> {
        let mut last_txid = None;
        let mut coins = Vec::new();
        let mut written = 0;
        let mut last_progress_update_time = Instant::now();

        snapshot_generator.write_metadata(bitcoin_block_hash, utxos_count)?;

        for entry in db.iterator(rocksdb::IteratorMode::Start) {
            let (key, value) = entry.unwrap();

            if key.as_ref() == COUNT_KEY {
                continue; // Skip the count key
            }

            let txid = Txid::from_slice(&key[0..32]).unwrap(); // First 32 bytes
            let vout = u32::from_be_bytes(key[32..36].try_into().unwrap()); // Next 4 bytes
            let coin: Coin = bincode::deserialize(&value).unwrap();

            match last_txid {
                Some(x) => {
                    if x == txid {
                        coins.push(OutputEntry { vout, coin });
                    } else {
                        let coins_per_txid = std::mem::take(&mut coins);
                        written += coins_per_txid.len();
                        snapshot_generator.write_coins(x, coins_per_txid)?;

                        if last_progress_update_time.elapsed() > INTERVAL {
                            tracing::info!(
                                target: "snapcake",
                                "Writing snapshot progress: {written}/{utxos_count}",
                            );
                            last_progress_update_time = Instant::now();
                        }

                        last_txid.replace(txid);
                        coins.push(OutputEntry { vout, coin });
                    }
                }
                None => {
                    last_txid.replace(txid);
                    coins.push(OutputEntry { vout, coin });
                }
            }
        }

        if let Some(txid) = last_txid {
            if !coins.is_empty() {
                written += coins.len();
                snapshot_generator.write_coins(txid, coins)?;
                tracing::info!(
                    target: "snapcake",
                    "Writing snapshot progress: {written}/{utxos_count}",
                );
            }
        }

        Ok(())
    }
}

/// CSV representation of a UTXO entry.
#[derive(serde::Serialize, serde::Deserialize)]
struct UtxoCsvEntry {
    txid: bitcoin::Txid,
    vout: u32,
    is_coinbase: bool,
    amount: u64,
    height: u32,
    script_pubkey: String,
}

impl From<Utxo> for UtxoCsvEntry {
    fn from(utxo: Utxo) -> Self {
        let Utxo { txid, vout, coin } = utxo;
        Self {
            txid,
            vout,
            is_coinbase: coin.is_coinbase,
            amount: coin.amount,
            height: coin.height,
            script_pubkey: hex::encode(&coin.script_pubkey),
        }
    }
}

impl From<UtxoCsvEntry> for Utxo {
    fn from(csv_entry: UtxoCsvEntry) -> Self {
        let UtxoCsvEntry {
            txid,
            vout,
            is_coinbase,
            amount,
            height,
            script_pubkey,
        } = csv_entry;

        let coin = Coin {
            is_coinbase,
            amount,
            height,
            script_pubkey: hex::decode(script_pubkey).expect("Failed to decode script_pubkey"),
        };

        Self { txid, vout, coin }
    }
}

pub struct SnapshotManager {
    store: SnapshotStore,
    snapshot_generator: UtxoSnapshotGenerator,
    target_bitcoin_block_hash: BlockHash,
    snapshot_dir: PathBuf,
}

impl SnapshotManager {
    /// Tries to load an existing cursor without creating a new SnapshotManager.
    ///
    /// This is useful to check if resumable sync is possible before initializing.
    pub fn try_load_cursor(
        target_block_number: u32,
        target_bitcoin_block_hash: BlockHash,
        snapshot_base_dir: &Path,
    ) -> Option<StateSyncCursor> {
        let sync_target = format!("{target_block_number}_{target_bitcoin_block_hash}");
        let snapshot_dir = snapshot_base_dir.join(sync_target);
        let cursor_path = snapshot_dir.join(CURSOR_FILE);

        if !cursor_path.exists() {
            return None;
        }

        let file = match File::open(&cursor_path) {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!(
                    target: "snapcake",
                    "Failed to open cursor file: {e}"
                );
                return None;
            }
        };

        let mut reader = BufReader::new(file);
        let mut buffer = Vec::new();
        if let Err(e) = reader.read_to_end(&mut buffer) {
            tracing::warn!(
                target: "snapcake",
                "Failed to read cursor file: {e}"
            );
            return None;
        }

        match bincode::deserialize(&buffer) {
            Ok(cursor) => Some(cursor),
            Err(e) => {
                tracing::warn!(
                    target: "snapcake",
                    "Failed to deserialize cursor file: {e}"
                );
                None
            }
        }
    }

    pub fn new(
        target_block_number: u32,
        target_bitcoin_block_hash: BlockHash,
        snapshot_base_dir: PathBuf,
        use_rocksdb: bool,
    ) -> Self {
        let sync_target = format!("{target_block_number}_{target_bitcoin_block_hash}");
        let snapshot_dir = snapshot_base_dir.join(sync_target);

        // Ensure the snapshot directory exists, creating it if necessary
        std::fs::create_dir_all(&snapshot_dir).expect("Failed to create snapshot directory");

        let snapshot_filepath = snapshot_dir.join("snapshot.dat");
        let snapshot_file =
            std::fs::File::create(&snapshot_filepath).expect("Failed to create output file");
        let snapshot_generator =
            UtxoSnapshotGenerator::new(snapshot_filepath, snapshot_file, bitcoin::Network::Bitcoin);

        let store = if use_rocksdb {
            let db = DB::open_default(snapshot_dir.join("db")).expect("Failed to open Rocksdb");
            SnapshotStore::Rocksdb(db)
        } else {
            let utxo_filepath = snapshot_dir.join("utxo.csv");

            // Open in write mode to clear existing content, then immediately close.
            std::fs::File::create(&utxo_filepath).expect("Failed to clear utxo.csv");
            SnapshotStore::Csv(utxo_filepath)
        };

        Self {
            store,
            snapshot_generator,
            target_bitcoin_block_hash,
            snapshot_dir,
        }
    }

    /// Returns the path to the cursor file.
    pub fn cursor_path(&self) -> PathBuf {
        self.snapshot_dir.join(CURSOR_FILE)
    }

    /// Saves the sync cursor to disk.
    ///
    /// This allows resuming state sync from the current position after a restart.
    pub fn save_cursor(&self, cursor: &StateSyncCursor) -> std::io::Result<()> {
        let cursor_path = self.cursor_path();
        let file = File::create(&cursor_path)?;
        let mut writer = BufWriter::new(file);

        let encoded =
            bincode::serialize(cursor).map_err(|e| std::io::Error::other(format!("{e}")))?;
        writer.write_all(&encoded)?;
        writer.flush()?;

        Ok(())
    }

    /// Loads the sync cursor from disk if it exists.
    ///
    /// Returns `None` if no cursor file exists or if it's corrupted.
    pub fn load_cursor(&self) -> std::io::Result<Option<StateSyncCursor>> {
        let cursor_path = self.cursor_path();

        if !cursor_path.exists() {
            return Ok(None);
        }

        let file = File::open(&cursor_path)?;
        let mut reader = BufReader::new(file);
        let mut buffer = Vec::new();
        reader.read_to_end(&mut buffer)?;

        match bincode::deserialize(&buffer) {
            Ok(cursor) => Ok(Some(cursor)),
            Err(e) => {
                tracing::warn!(
                    target: "snapcake",
                    "Failed to deserialize cursor file, starting fresh: {e}"
                );
                Ok(None)
            }
        }
    }

    /// Clears the sync cursor from disk.
    ///
    /// Called when state sync completes successfully.
    pub fn clear_cursor(&self) -> std::io::Result<()> {
        let cursor_path = self.cursor_path();

        if cursor_path.exists() {
            std::fs::remove_file(&cursor_path)?;
            tracing::info!(
                target: "snapcake",
                "Cleared state sync cursor"
            );
        }

        Ok(())
    }

    /// Returns the current count of stored coins.
    pub fn stored_coin_count(&self) -> std::io::Result<usize> {
        self.store.count()
    }

    /// Adds a batch of UTXOs to the store.
    ///
    /// Returns the number of UTXOs stored.
    pub fn store_utxos_batch(&mut self, utxos: &[Utxo]) -> usize {
        self.store
            .push_batch(utxos)
            .expect("Failed to add UTXOs batch to the store")
    }

    /// Generates a snapshot and ensures the total UTXO count matches the expected count.
    pub fn create_snapshot(&mut self, expected_utxos_count: usize) -> std::io::Result<()> {
        let utxos_count = self
            .store
            .count()
            .expect("Failed to calculate the count of stored UTXO");

        assert_eq!(utxos_count, expected_utxos_count, "UTXO count mismatches");

        tracing::info!(
            target: "snapcake",
            "Writing the snapshot to {}",
            self.snapshot_generator.path().display()
        );

        self.store.generate_snapshot(
            &mut self.snapshot_generator,
            self.target_bitcoin_block_hash,
            utxos_count as u64,
        )?;

        if let Some(expected_sha256sum) =
            MAINNET_SNAPSHOT_SHA256SUMS.get(&self.target_bitcoin_block_hash)
        {
            let sha256sum = calculate_sha256sum(self.snapshot_generator.path())?;

            if sha256sum != *expected_sha256sum {
                panic!(
                    "Invalid snapshot sha256sum for block#{} (expected: {expected_sha256sum}, got: {sha256sum})",
                    self.target_bitcoin_block_hash,
                );
            }
        }

        Ok(())
    }
}

/// Calculates the SHA256 checksum of a file.
fn calculate_sha256sum(file_path: impl AsRef<Path>) -> std::io::Result<String> {
    let mut reader = BufReader::new(File::open(file_path)?);

    // Create a SHA256 hasher
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 4096]; // Read the file in chunks

    // Read file and update the hasher
    while let Ok(bytes_read) = reader.read(&mut buffer) {
        if bytes_read == 0 {
            break; // End of file
        }
        hasher.update(&buffer[..bytes_read]);
    }

    // Finalize the hash and return it as a hexadecimal string
    let hash_result = hasher.finalize();

    Ok(format!("{hash_result:x}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_block_hash() -> BlockHash {
        "000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f"
            .parse()
            .expect("Valid genesis block hash")
    }

    fn test_block_hash_2() -> BlockHash {
        "00000000839a8e6886ab5951d76f411475428afc90947ee320161bbf18eb6048"
            .parse()
            .expect("Valid block 1 hash")
    }

    fn create_test_cursor(
        target_block_number: u32,
        target_bitcoin_block_hash: BlockHash,
        received_coins: u64,
    ) -> StateSyncCursor {
        StateSyncCursor {
            target_block_number,
            target_bitcoin_block_hash,
            last_key: vec![vec![1, 2, 3, 4], vec![5, 6, 7, 8]],
            total_coins: 1000,
            received_coins,
        }
    }

    #[test]
    fn cursor_save_and_load_roundtrip() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let block_hash = test_block_hash();
        let block_number = 100u32;

        let manager = SnapshotManager::new(block_number, block_hash, temp_dir.path().into(), false);

        // Create and save cursor
        let original_cursor = create_test_cursor(block_number, block_hash, 500);
        manager
            .save_cursor(&original_cursor)
            .expect("Failed to save cursor");

        // Load cursor and verify
        let loaded_cursor = manager
            .load_cursor()
            .expect("Failed to load cursor")
            .expect("Cursor should exist");

        assert_eq!(
            loaded_cursor.target_block_number,
            original_cursor.target_block_number
        );
        assert_eq!(
            loaded_cursor.target_bitcoin_block_hash,
            original_cursor.target_bitcoin_block_hash
        );
        assert_eq!(loaded_cursor.last_key, original_cursor.last_key);
        assert_eq!(loaded_cursor.total_coins, original_cursor.total_coins);
        assert_eq!(loaded_cursor.received_coins, original_cursor.received_coins);
    }

    #[test]
    fn cursor_clear_removes_file() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let block_hash = test_block_hash();
        let block_number = 100u32;

        let manager = SnapshotManager::new(block_number, block_hash, temp_dir.path().into(), false);

        // Save cursor
        let cursor = create_test_cursor(block_number, block_hash, 500);
        manager.save_cursor(&cursor).expect("Failed to save cursor");

        // Verify cursor exists
        assert!(manager.cursor_path().exists());

        // Clear cursor
        manager.clear_cursor().expect("Failed to clear cursor");

        // Verify cursor is gone
        assert!(!manager.cursor_path().exists());

        // Load should return None
        let loaded = manager.load_cursor().expect("Failed to load cursor");
        assert!(loaded.is_none());
    }

    #[test]
    fn load_cursor_returns_none_when_no_file() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let block_hash = test_block_hash();
        let block_number = 100u32;

        let manager = SnapshotManager::new(block_number, block_hash, temp_dir.path().into(), false);

        // Should return None without error
        let loaded = manager.load_cursor().expect("Failed to load cursor");
        assert!(loaded.is_none());
    }

    #[test]
    fn try_load_cursor_without_manager() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let block_hash = test_block_hash();
        let block_number = 100u32;

        // Create manager and save cursor
        let manager = SnapshotManager::new(block_number, block_hash, temp_dir.path().into(), false);
        let cursor = create_test_cursor(block_number, block_hash, 500);
        manager.save_cursor(&cursor).expect("Failed to save cursor");

        // Now use static method to load without creating a new manager
        let loaded = SnapshotManager::try_load_cursor(block_number, block_hash, temp_dir.path());

        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.target_block_number, block_number);
        assert_eq!(loaded.target_bitcoin_block_hash, block_hash);
        assert_eq!(loaded.received_coins, 500);
    }

    #[test]
    fn try_load_cursor_returns_none_for_different_target() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let block_hash = test_block_hash();
        let block_number = 100u32;

        // Create manager and save cursor for block 100
        let manager = SnapshotManager::new(block_number, block_hash, temp_dir.path().into(), false);
        let cursor = create_test_cursor(block_number, block_hash, 500);
        manager.save_cursor(&cursor).expect("Failed to save cursor");

        // Try to load cursor for block 200 - should return None
        let loaded = SnapshotManager::try_load_cursor(200, block_hash, temp_dir.path());
        assert!(loaded.is_none());

        // Try to load cursor for different block hash - should return None
        let different_hash = test_block_hash_2();
        let loaded =
            SnapshotManager::try_load_cursor(block_number, different_hash, temp_dir.path());
        assert!(loaded.is_none());
    }

    #[test]
    fn cursor_update_overwrites_previous() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let block_hash = test_block_hash();
        let block_number = 100u32;

        let manager = SnapshotManager::new(block_number, block_hash, temp_dir.path().into(), false);

        // Save initial cursor
        let cursor1 = create_test_cursor(block_number, block_hash, 100);
        manager
            .save_cursor(&cursor1)
            .expect("Failed to save cursor");

        // Save updated cursor with more progress
        let cursor2 = StateSyncCursor {
            target_block_number: block_number,
            target_bitcoin_block_hash: block_hash,
            last_key: vec![vec![10, 20, 30]],
            total_coins: 1000,
            received_coins: 500,
        };
        manager
            .save_cursor(&cursor2)
            .expect("Failed to save cursor");

        // Load should return the updated cursor
        let loaded = manager
            .load_cursor()
            .expect("Failed to load cursor")
            .expect("Cursor should exist");

        assert_eq!(loaded.received_coins, 500);
        assert_eq!(loaded.last_key, vec![vec![10, 20, 30]]);
    }

    #[test]
    fn cursor_serialization_handles_empty_last_key() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let block_hash = test_block_hash();
        let block_number = 100u32;

        let manager = SnapshotManager::new(block_number, block_hash, temp_dir.path().into(), false);

        // Create cursor with empty last_key (initial state)
        let cursor = StateSyncCursor {
            target_block_number: block_number,
            target_bitcoin_block_hash: block_hash,
            last_key: vec![],
            total_coins: 1000,
            received_coins: 0,
        };
        manager.save_cursor(&cursor).expect("Failed to save cursor");

        let loaded = manager
            .load_cursor()
            .expect("Failed to load cursor")
            .expect("Cursor should exist");

        assert!(loaded.last_key.is_empty());
        assert_eq!(loaded.received_coins, 0);
    }

    #[test]
    fn clear_cursor_is_idempotent() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let block_hash = test_block_hash();
        let block_number = 100u32;

        let manager = SnapshotManager::new(block_number, block_hash, temp_dir.path().into(), false);

        // Clear cursor when it doesn't exist - should not error
        manager.clear_cursor().expect("First clear should succeed");
        manager.clear_cursor().expect("Second clear should succeed");
        manager.clear_cursor().expect("Third clear should succeed");
    }

    #[test]
    fn stored_coin_count_tracks_utxos() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let block_hash = test_block_hash();
        let block_number = 100u32;

        let mut manager =
            SnapshotManager::new(block_number, block_hash, temp_dir.path().into(), true);

        // Initial count should be 0
        let count = manager
            .stored_coin_count()
            .expect("Failed to get coin count");
        assert_eq!(count, 0);

        // Add some UTXOs
        let utxos = vec![
            Utxo {
                txid: "0000000000000000000000000000000000000000000000000000000000000001"
                    .parse()
                    .unwrap(),
                vout: 0,
                coin: Coin {
                    is_coinbase: true,
                    amount: 5000000000,
                    height: 1,
                    script_pubkey: vec![0x76, 0xa9],
                },
            },
            Utxo {
                txid: "0000000000000000000000000000000000000000000000000000000000000002"
                    .parse()
                    .unwrap(),
                vout: 0,
                coin: Coin {
                    is_coinbase: true,
                    amount: 5000000000,
                    height: 2,
                    script_pubkey: vec![0x76, 0xa9],
                },
            },
        ];

        let stored = manager.store_utxos_batch(&utxos);
        assert_eq!(stored, 2);

        let count = manager
            .stored_coin_count()
            .expect("Failed to get coin count");
        assert_eq!(count, 2);

        // Add more UTXOs
        let more_utxos = vec![Utxo {
            txid: "0000000000000000000000000000000000000000000000000000000000000003"
                .parse()
                .unwrap(),
            vout: 0,
            coin: Coin {
                is_coinbase: true,
                amount: 5000000000,
                height: 3,
                script_pubkey: vec![0x76, 0xa9],
            },
        }];

        manager.store_utxos_batch(&more_utxos);

        let count = manager
            .stored_coin_count()
            .expect("Failed to get coin count");
        assert_eq!(count, 3);
    }
}
