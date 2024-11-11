use codec::Decode;
use sc_client_api::ProofProvider;
use sc_network_sync::strategy::state_sync::{
    ImportResult, StateSync, StateSyncProgress, StateSyncProvider,
};
use sc_network_sync::{StateRequest, StateResponse};
use sp_runtime::traits::{Block as BlockT, Header as HeaderT, NumberFor};
use std::path::PathBuf;
use std::sync::Arc;
use subcoin_crypto::muhash::MuHash3072;
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

enum UtxoStore {
    InMem(Vec<Utxo>),
    Csv(std::fs::File),
}

impl UtxoStore {
    fn push(&mut self, utxo: Utxo) {
        match self {
            Self::InMem(list) => list.push(utxo),
            Self::Csv(file) => {
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
            Self::Csv(file) => {
                // Clone the file handle for counting to avoid interfering with write access.
                let reader = std::io::BufReader::new(file.try_clone()?);
                let mut csv_reader = csv::Reader::from_reader(reader);

                // Count all valid rows
                let count = csv_reader.records().count();
                Ok(count)
            }
        }
    }
}

/// Wrapped [`StateSync`] to intercept the state response for parsing the UTXO state.
pub(crate) struct StateSyncWrapper<B: BlockT, Client> {
    inner: StateSync<B, Client>,
    muhash: MuHash3072,
    target_bitcoin_block_hash: bitcoin::BlockHash,
    utxo_store: UtxoStore,
    snapshot_generator: UtxoSnapshotGenerator,
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
        snapshot_dir: PathBuf,
    ) -> Self {
        let target_block_number = target_header.number();
        let target_bitcoin_block_hash =
            subcoin_primitives::extract_bitcoin_block_hash::<B>(&target_header)
                .expect("Failed to extract bitcoin block hash");

        let file_name = format!("{target_block_number}_{target_bitcoin_block_hash}_snapshot.dat");
        let snapshot_file = snapshot_dir.join(file_name);
        let snapshot_generator = UtxoSnapshotGenerator::new(
            std::fs::File::create(snapshot_file).expect("Failed to create output file"),
        );

        let utxo_file_name = format!("{target_block_number}_{target_bitcoin_block_hash}.utxo");
        let utxo_file = snapshot_dir.join(utxo_file_name);
        let utxo_file = std::fs::File::create(utxo_file).expect("Failed to create UTXO file");
        Self {
            inner: StateSync::new(client, target_header, None, None, skip_proof),
            target_bitcoin_block_hash,
            muhash: MuHash3072::new(),
            utxo_store: UtxoStore::Csv(utxo_file),
            snapshot_generator,
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
                }
            }
        }

        if complete {
            let muhash = self.muhash.txoutset_muhash();

            let utxos_count = self.utxo_store.count().unwrap() as u64;

            tracing::info!(
                muhash,
                utxos_count,
                "💾 State dowload is complete, writing the UTXO snapshot"
            );

            match &mut self.utxo_store {
                UtxoStore::InMem(list) => {
                    let utxos = std::mem::take(list);

                    self.snapshot_generator
                        .write_utxo_snapshot(self.target_bitcoin_block_hash, utxos_count, utxos)
                        .expect("Failed to write UTXO set snapshot");
                }
                UtxoStore::Csv(file) => {
                    let reader = std::io::BufReader::new(file.try_clone().unwrap()); // Clone file handle for reading
                    let csv_reader = csv::Reader::from_reader(reader);

                    let utxo_iter = csv_reader
                        .into_deserialize::<UtxoCsvEntry>()
                        .filter_map(Result::ok)
                        .map(|csv_entry| {
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
                                script_pubkey: hex::decode(script_pubkey)
                                    .expect("Failed to decode script_pubkey"),
                            };

                            Utxo { txid, vout, coin }
                        });

                    self.snapshot_generator
                        .write_utxo_snapshot(self.target_bitcoin_block_hash, utxos_count, utxo_iter)
                        .expect("Failed to write UTXO set snapshot");
                }
            }

            tracing::info!("UTXO set snapshot has been generated successfully!");
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
