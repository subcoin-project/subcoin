use bitcoin::BlockHash;
use std::path::PathBuf;
use subcoin_runtime_primitives::Coin;
use subcoin_utxo_snapshot::{Utxo, UtxoSnapshotGenerator};

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
    Csv(PathBuf),
    // Store the coins in lexicographical order.
    // Rocksdb(PathBuf),
}

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
        }
    }

    fn write_snapshot(
        &mut self,
        snapshot_generator: &mut UtxoSnapshotGenerator,
        target_bitcoin_block_hash: BlockHash,
        utxos_count: usize,
    ) -> std::io::Result<()> {
        match self {
            UtxoStore::InMem(list) => {
                let utxos = std::mem::take(list);

                snapshot_generator.write_utxo_snapshot(
                    target_bitcoin_block_hash,
                    utxos_count as u64,
                    utxos,
                )?;
            }
            UtxoStore::Csv(path) => {
                let file = std::fs::File::open(path).expect("Failed to open utxo.csv");

                let reader = std::io::BufReader::new(file);
                let csv_reader = csv::ReaderBuilder::new()
                    .has_headers(false)
                    .from_reader(reader);

                let utxo_iter = csv_reader
                    .into_deserialize::<UtxoCsvEntry>()
                    .filter_map(Result::ok)
                    .map(Utxo::from);

                snapshot_generator.write_utxo_snapshot(
                    target_bitcoin_block_hash,
                    utxos_count as u64,
                    utxo_iter,
                )?;
            }
        }

        Ok(())
    }
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

        let utxo_filepath = snapshot_dir.join("utxo.csv");

        // Open in write mode to clear existing content, then immediately close.
        std::fs::File::create(&utxo_filepath).expect("Failed to clear utxo.csv");

        Self {
            store: UtxoStore::Csv(utxo_filepath),
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
