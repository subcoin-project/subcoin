use bitcoin::hashes::Hash;
use bitcoin::{BlockHash, Txid};
use rocksdb::DB;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use subcoin_runtime_primitives::Coin;
use subcoin_utxo_snapshot::{OutputEntry, Utxo, UtxoSnapshotGenerator};

const COUNT_KEY: &[u8; 12] = b"__coin_count";
const INTERVAL: Duration = Duration::from_secs(5);

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
    /// Append a UTXO to the store.
    fn push(&mut self, utxo: Utxo) -> std::io::Result<()> {
        match self {
            Self::InMem(list) => {
                list.push(utxo);
                Ok(())
            }
            Self::Csv(path) => Self::append_to_csv(path, utxo),
            Self::Rocksdb(db) => Self::insert_to_rocksdb(db, utxo).map_err(|err| {
                std::io::Error::new(std::io::ErrorKind::Other, format!("rocksdb: {err:?}"))
            }),
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
            panic!("Failed to write UTXO entry to CSV: {e}");
        }

        wtr.flush()?;

        Ok(())
    }

    fn insert_to_rocksdb(db: &DB, utxo: Utxo) -> Result<(), rocksdb::Error> {
        let mut coin_count =
            Self::read_rocksdb_count(db).expect("Failed to read count from Rocksdb") as u64;

        let Utxo { txid, vout, coin } = utxo;

        let mut key = Vec::with_capacity(32 + 4);
        key.extend(txid.to_byte_array()); // Raw bytes of Txid
        key.extend(vout.to_be_bytes()); // Ensure vout is big-endian for consistent ordering

        let value = bincode::serialize(&coin).expect("Failed to serialize Coin"); // Serialize coin data

        coin_count += 1;

        db.put(&key, value)?;
        db.put(COUNT_KEY, coin_count.to_le_bytes())?;

        Ok(())
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
                utxos_count as u64,
                std::mem::take(list),
            ),
            Self::Csv(path) => generate_from_csv(
                path,
                snapshot_generator,
                target_bitcoin_block_hash,
                utxos_count,
            ),
            Self::Rocksdb(db) => generate_from_rocksdb(
                db,
                snapshot_generator,
                target_bitcoin_block_hash,
                utxos_count,
            ),
        }
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

pub struct SnapshotProcessor {
    store: SnapshotStore,
    snapshot_generator: UtxoSnapshotGenerator,
    target_bitcoin_block_hash: BlockHash,
}

impl SnapshotProcessor {
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
        }
    }

    /// Adds a UTXO to the store.
    pub fn store_utxo(&mut self, utxo: Utxo) {
        self.store
            .push(utxo)
            .expect("Failed to add UTXO to the store");
    }

    /// Generates a snapshot and ensures the total UTXO count matches the expected count.
    pub fn create_snapshot(&mut self, expected_utxos_count: usize) {
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

        self.store
            .generate_snapshot(
                &mut self.snapshot_generator,
                self.target_bitcoin_block_hash,
                utxos_count as u64,
            )
            .expect("Failed to write UTXO set snapshot");
    }
}
