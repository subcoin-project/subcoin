use codec::Decode;
use sc_client_api::ProofProvider;
use sc_network_sync::strategy::state_sync::{
    ImportResult, StateSync, StateSyncProgress, StateSyncProvider,
};
use sc_network_sync::{StateRequest, StateResponse};
use sp_runtime::traits::{Block as BlockT, Header as HeaderT, NumberFor};
use sp_runtime::SaturatedConversion;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use subcoin_crypto::muhash::MuHash3072;
use subcoin_runtime_primitives::Coin;
use subcoin_utxo_snapshot::{Utxo, UtxoSnapshotGenerator};

const DOWNLOAD_PROGRESS_LOG_INTERVAL: Duration = Duration::from_secs(5);

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
    InMem(Vec<Utxo>),
    Csv(PathBuf),
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
}

/// Wrapped [`StateSync`] to intercept the state response for parsing the UTXO state.
pub(crate) struct StateSyncWrapper<B: BlockT, Client> {
    inner: StateSync<B, Client>,
    muhash: MuHash3072,
    target_block_number: u32,
    target_bitcoin_block_hash: bitcoin::BlockHash,
    utxo_store: UtxoStore,
    snapshot_generator: UtxoSnapshotGenerator,
    received_coins: usize,
    total_coins: usize,
    last_progress_print_time: Option<Instant>,
}

impl<B, Client> StateSyncWrapper<B, Client>
where
    B: BlockT,
    Client: ProofProvider<B> + Send + Sync + 'static,
{
    pub(crate) fn new(
        client: Arc<Client>,
        target_header: B::Header,
        skip_proof: bool,
        snapshot_base_dir: PathBuf,
        total_coins: usize,
    ) -> Self {
        let target_block_number = *target_header.number();
        let target_bitcoin_block_hash =
            subcoin_primitives::extract_bitcoin_block_hash::<B>(&target_header)
                .expect("Failed to extract bitcoin block hash");

        let sync_target = format!("{target_block_number}_{target_bitcoin_block_hash}");
        let snapshot_dir = snapshot_base_dir.join(sync_target);

        // Ensure the snapshot directory exists, creating it if necessary
        std::fs::create_dir_all(&snapshot_dir).expect("Failed to create snapshot directory");

        let snapshot_filepath = snapshot_dir.join("snapshot.dat");
        let snapshot_file =
            std::fs::File::create(&snapshot_filepath).expect("Failed to create output file");
        let snapshot_generator = UtxoSnapshotGenerator::new(snapshot_filepath, snapshot_file);

        let utxo_filepath = snapshot_dir.join("utxo.csv");

        // Open in write mode to clear existing content, then immediately close.
        std::fs::File::create(&utxo_filepath).expect("Failed to clear utxo.csv");

        Self {
            inner: StateSync::new(client, target_header, None, None, skip_proof),
            target_block_number: target_block_number.saturated_into(),
            target_bitcoin_block_hash,
            muhash: MuHash3072::new(),
            utxo_store: UtxoStore::Csv(utxo_filepath),
            snapshot_generator,
            received_coins: 0,
            total_coins,
            last_progress_print_time: None,
        }
    }

    // Find the Coin storage key values and store them locally.
    fn process_state_response(&mut self, response: &StateResponse) {
        let mut complete = false;

        let key_values = response.entries.iter().flat_map(|key_vlaue_state_entry| {
            if key_vlaue_state_entry.complete {
                complete = true;
            }

            key_vlaue_state_entry
                .entries
                .iter()
                .map(|state_entry| (&state_entry.key, &state_entry.value))
        });

        for (key, value) in key_values {
            if key.len() > 32 {
                // Attempt to decode the Coin storage item.
                if let Ok((txid, vout)) =
                    <(pallet_bitcoin::types::Txid, u32)>::decode(&mut &key.as_slice()[32..])
                {
                    let txid = txid.into_bitcoin_txid();

                    let coin = Coin::decode(&mut value.as_slice())
                        .expect("Coin in state response must be decoded successfully; qed");

                    let data =
                        subcoin_utxo_snapshot::tx_out_ser(bitcoin::OutPoint { txid, vout }, &coin)
                            .expect("Failed to serialize txout");

                    self.muhash.insert(&data);

                    // TODO: write UTXO to a local file instead of storing in memory.
                    self.utxo_store.push(Utxo { txid, vout, coin });
                    self.received_coins += 1;
                }
            }
        }

        if self.last_progress_print_time.map_or(true, |last_time| {
            last_time.elapsed() > DOWNLOAD_PROGRESS_LOG_INTERVAL
        }) {
            let percent = self.received_coins as f64 * 100.0 / self.total_coins as f64;
            tracing::info!(target: "snapcake", "Download progress: {percent:.2}%");
            self.last_progress_print_time.replace(Instant::now());
        }

        if complete {
            let muhash = self.muhash.txoutset_muhash();

            let utxos_count = self
                .utxo_store
                .count()
                .expect("Failed to calculate the count of stored UTXO");

            assert_eq!(utxos_count, self.total_coins, "UTXO count mismatches");

            tracing::info!(
                target: "snapcake",
                %muhash,
                "💾 State dowload is complete ({}/{})",
                self.received_coins,
                self.total_coins,
            );

            tracing::info!(
                target: "snapcake",
                "Writing the snapshot to {}",
                self.snapshot_generator.path().display()
            );

            match &mut self.utxo_store {
                UtxoStore::InMem(list) => {
                    let utxos = std::mem::take(list);

                    self.snapshot_generator
                        .write_utxo_snapshot(
                            self.target_bitcoin_block_hash,
                            utxos_count as u64,
                            utxos,
                        )
                        .expect("Failed to write UTXO set snapshot");
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

                    self.snapshot_generator
                        .write_utxo_snapshot(
                            self.target_bitcoin_block_hash,
                            utxos_count as u64,
                            utxo_iter,
                        )
                        .expect("Failed to write UTXO set snapshot");
                }
            }

            tracing::info!(
                target: "snapcake",
                "UTXO snapshot at Bitcoin block #{},{} has been generated successfully!",
                self.target_block_number,
                self.target_bitcoin_block_hash
            );
        }
    }
}

impl<B, Client> StateSyncProvider<B> for StateSyncWrapper<B, Client>
where
    B: BlockT,
    Client: ProofProvider<B> + Send + Sync + 'static,
{
    fn import(&mut self, response: StateResponse) -> ImportResult<B> {
        self.process_state_response(&response);
        self.inner.import(response)
    }
    fn next_request(&self) -> StateRequest {
        self.inner.next_request()
    }
    fn is_complete(&self) -> bool {
        self.inner.is_complete()
    }
    fn target_number(&self) -> NumberFor<B> {
        self.inner.target_number()
    }
    fn target_hash(&self) -> B::Hash {
        self.inner.target_hash()
    }
    fn progress(&self) -> StateSyncProgress {
        self.inner.progress()
    }
}
