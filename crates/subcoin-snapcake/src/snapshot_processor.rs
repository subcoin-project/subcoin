use bitcoin::{hashes::Hash, BlockHash, Txid};
use rocksdb::DB;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use subcoin_runtime_primitives::Coin;
use subcoin_utxo_snapshot::{OutputEntry, Utxo, UtxoSnapshotGenerator};

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

enum UtxoStore {
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

const COUNT_KEY: &[u8; 12] = b"__coin_count";

impl UtxoStore {
    fn push(&mut self, utxo: Utxo) {
        match self {
            Self::InMem(list) => list.push(utxo),
            Self::Csv(path) => {
                // Open the file in append mode each time to avoid keeping it open across calls
                let file = std::fs::OpenOptions::new()
                    .append(true)
                    .open(path)
                    .expect("Failed to open file");

                let mut wtr = csv::WriterBuilder::new()
                    .has_headers(false) // Disable automatic header writing
                    .from_writer(file);

                let utxo_csv_entry = UtxoCsvEntry::from(utxo);
                if let Err(e) = wtr.serialize(&utxo_csv_entry) {
                    panic!("Failed to write UTXO entry to CSV: {}", e);
                }

                if let Err(e) = wtr.flush() {
                    panic!("Failed to flush CSV writer: {}", e);
                }
            }
            Self::Rocksdb(db) => {
                let Utxo { txid, vout, coin } = utxo;
                let mut coin_count: u64 = db
                    .get(COUNT_KEY)
                    .expect("Failed to read CoinCount from Rocksdb")
                    .map(|v| u64::from_le_bytes(v.try_into().expect("Invalid value")))
                    .unwrap_or(0);

                let mut key = Vec::with_capacity(32 + 4);
                key.extend_from_slice(txid.as_byte_array()); // Raw bytes of Txid
                key.extend_from_slice(&vout.to_be_bytes()); // Ensure vout is big-endian for consistent ordering

                let value = bincode::serialize(&coin).expect("Failed to serialize Coin"); // Serialize coin data

                coin_count += 1;

                db.put(&key, value)
                    .expect("Failed to put Coin into Rocksdb");
                db.put(COUNT_KEY, coin_count.to_le_bytes())
                    .expect("Failed to push CoinCount into Rocksdb");
            }
        }
    }

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
            Self::Rocksdb(db) => Ok(db
                .get(COUNT_KEY)
                .expect("Failed to read CoinCount from Rocksdb")
                .map(|v| u64::from_le_bytes(v.try_into().expect("Invalid value")))
                .unwrap_or(0) as usize),
        }
    }

    fn write_snapshot(
        &mut self,
        snapshot_generator: &mut UtxoSnapshotGenerator,
        target_bitcoin_block_hash: BlockHash,
        utxos_count: usize,
    ) -> std::io::Result<()> {
        match self {
            Self::InMem(list) => {
                let utxos = std::mem::take(list);

                snapshot_generator.write_utxo_snapshot_in_memory(
                    target_bitcoin_block_hash,
                    utxos_count as u64,
                    utxos,
                )?;
            }
            Self::Csv(path) => {
                let file = std::fs::File::open(path).expect("Failed to open utxo.csv");

                let reader = std::io::BufReader::new(file);
                let csv_reader = csv::ReaderBuilder::new()
                    .has_headers(false)
                    .from_reader(reader);

                let utxo_iter = csv_reader
                    .into_deserialize::<UtxoCsvEntry>()
                    .filter_map(Result::ok)
                    .map(Utxo::from);

                snapshot_generator.write_utxo_snapshot_in_memory(
                    target_bitcoin_block_hash,
                    utxos_count as u64,
                    utxo_iter,
                )?;
            }
            Self::Rocksdb(db) => {
                generate_snapshot_from_rocksdb(
                    db,
                    snapshot_generator,
                    target_bitcoin_block_hash,
                    utxos_count,
                )?;
            }
        }

        Ok(())
    }
}

fn generate_snapshot_from_rocksdb(
    db: &DB,
    snapshot_generator: &mut UtxoSnapshotGenerator,
    bitcoin_block_hash: BlockHash,
    utxos_count: usize,
) -> std::io::Result<()> {
    snapshot_generator.write_snapshot_metadata(bitcoin_block_hash, utxos_count as u64)?;

    let iter = db.iterator(rocksdb::IteratorMode::Start);

    let mut last_txid = None;
    let mut coins = Vec::new();

    let mut written = 0;

    const INTERVAL: Duration = Duration::from_secs(5);
    let mut last_progress_update_time = Instant::now();

    for entry in iter {
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
                    let batch = std::mem::take(&mut coins);
                    written += batch.len();
                    snapshot_generator.write_coins(x, batch)?;

                    if last_progress_update_time.elapsed() > INTERVAL {
                        tracing::info!(
                            target: "snapcake",
                            "Writing snapshot progress: {written}/{utxos_count}",
                        );
                        println!(
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

    if !coins.is_empty() {
        let txid = last_txid.unwrap();
        written += coins.len();
        snapshot_generator.write_coins(txid, coins)?;
        tracing::info!(
            target: "snapcake",
            "Writing snapshot progress: {written}/{utxos_count}",
        );
    }

    Ok(())
}

pub struct SnapshotProcessor {
    store: UtxoStore,
    snapshot_generator: UtxoSnapshotGenerator,
    target_bitcoin_block_hash: BlockHash,
}

impl SnapshotProcessor {
    pub fn new(
        target_block_number: u32,
        target_bitcoin_block_hash: BlockHash,
        snapshot_base_dir: PathBuf,
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

        let rocksdb = true;

        let store = if rocksdb {
            let db = DB::open_default(snapshot_dir.join("db")).expect("Failed to open Rocksdb");
            UtxoStore::Rocksdb(db)
        } else {
            let utxo_filepath = snapshot_dir.join("utxo.csv");

            // Open in write mode to clear existing content, then immediately close.
            std::fs::File::create(&utxo_filepath).expect("Failed to clear utxo.csv");
            UtxoStore::Csv(utxo_filepath)
        };

        Self {
            store,
            snapshot_generator,
            target_bitcoin_block_hash,
        }
    }

    pub fn store_utxo(&mut self, utxo: Utxo) {
        self.store.push(utxo);
    }

    pub fn write_snapshot(&mut self, expected_utxos_count: usize) {
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
            .write_snapshot(
                &mut self.snapshot_generator,
                self.target_bitcoin_block_hash,
                utxos_count,
            )
            .expect("Failed to write UTXO set snapshot");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rocksdb() {
        let snapshot_dir = PathBuf::from("/home/xlc/Work/src/github.com/subcoin-project/subcoin/snapshots/840000_0000000000000000000320283a032748cef8227873ff4872689bf23f1cda83a5");
        let db = DB::open_default(snapshot_dir.join("db")).expect("Failed to open Rocksdb");

        let txid: bitcoin::Txid =
            "7e27944eacef755d2fa83b6559f480941aa4f0e5f3f0623fc3d0de9cd28c0100"
                .parse()
                .unwrap();
        let vout = 1u32;

        let mut key = Vec::with_capacity(32 + 4);
        key.extend_from_slice(txid.as_byte_array()); // Raw bytes of Txid
        key.extend_from_slice(&vout.to_be_bytes()); // Ensure vout is big-endian for consistent ordering

        let value = db.get(&key).unwrap().unwrap();
        let coin: Coin = bincode::deserialize(&value).unwrap();
        println!("====== Coin: {coin:02x?}");

        let mut data = Vec::new();

        subcoin_utxo_snapshot::write_coins(&mut data, txid, vec![OutputEntry { vout, coin }])
            .unwrap();
        println!("seralized bytes: {:02x?}", data);
    }

    #[test]
    fn regenerate_snapshot() {
        let snapshot_dir = PathBuf::from("/home/xlc/Work/src/github.com/subcoin-project/subcoin/snapshots/840000_0000000000000000000320283a032748cef8227873ff4872689bf23f1cda83a5");
        let db = DB::open_default(snapshot_dir.join("db")).expect("Failed to open Rocksdb");

        let utxos_count = 176948713;

        let snapshot_filepath = PathBuf::from("/tmp/snapshot.dat");
        let snapshot_file =
            std::fs::File::create(&snapshot_filepath).expect("Failed to create output file");
        let mut snapshot_generator =
            UtxoSnapshotGenerator::new(snapshot_filepath, snapshot_file, bitcoin::Network::Bitcoin);

        let block_hash: BlockHash =
            "0000000000000000000320283a032748cef8227873ff4872689bf23f1cda83a5"
                .parse()
                .unwrap();
        generate_snapshot_from_rocksdb(&db, &mut snapshot_generator, block_hash, utxos_count)
            .unwrap();
    }
}

