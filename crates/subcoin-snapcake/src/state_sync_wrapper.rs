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
use subcoin_utils::{Utxo, UtxoSetGenerator};

/// Wrapped [`StateSync`] to intercept the state response for parsing the UTXO state.
pub(crate) struct StateSyncWrapper<B: BlockT, Client> {
    inner: StateSync<B, Client>,
    muhash: MuHash3072,
    target_bitcoin_block_hash: bitcoin::BlockHash,
    utxos: Vec<Utxo>,
    utxo_set_generator: UtxoSetGenerator,
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
        let utxo_set_generator = UtxoSetGenerator::new(
            std::fs::File::create(snapshot_file).expect("Failed to create output file"),
        );

        Self {
            inner: StateSync::new(client, target_header, None, None, skip_proof),
            target_bitcoin_block_hash,
            muhash: MuHash3072::new(),
            utxos: Vec::new(),
            utxo_set_generator,
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
                        subcoin_primitives::tx_out_ser(bitcoin::OutPoint { txid, vout }, &coin)
                            .expect("Failed to serialize txout");

                    self.muhash.insert(&data);

                    // TODO: write UTXO to a local file instead of storing in memory.
                    self.utxos.push(Utxo { txid, vout, coin });
                }
            }
        }

        if complete {
            let muhash = self.muhash.txoutset_muhash();

            let utxos = std::mem::take(&mut self.utxos);

            let utxos_count = utxos.len();

            tracing::info!(
                muhash,
                utxos_count,
                "💾 State dowload is complete, writing the UTXO snapshot"
            );

            self.utxo_set_generator
                .write_utxo_snapshot(self.target_bitcoin_block_hash, utxos_count as u64, utxos)
                .expect("Failed to write binary output");
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
