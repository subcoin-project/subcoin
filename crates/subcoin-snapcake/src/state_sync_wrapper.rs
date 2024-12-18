use crate::snapshot_manager::SnapshotManager;
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
use subcoin_utxo_snapshot::Utxo;

const DOWNLOAD_PROGRESS_LOG_INTERVAL: Duration = Duration::from_secs(5);

/// Wrapped [`StateSync`] to intercept the state response for parsing the UTXO state.
pub(crate) struct StateSyncWrapper<B: BlockT, Client> {
    inner: StateSync<B, Client>,
    muhash: MuHash3072,
    target_block_number: u32,
    target_bitcoin_block_hash: bitcoin::BlockHash,
    snapshot_manager: SnapshotManager,
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

        let snapshot_manager = SnapshotManager::new(
            target_block_number.saturated_into(),
            target_bitcoin_block_hash,
            snapshot_base_dir,
            true,
        );

        Self {
            inner: StateSync::new(client, target_header, None, None, skip_proof),
            target_block_number: target_block_number.saturated_into(),
            target_bitcoin_block_hash,
            muhash: MuHash3072::new(),
            snapshot_manager,
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

                    self.snapshot_manager.store_utxo(Utxo { txid, vout, coin });

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

            tracing::info!(
                target: "snapcake",
                %muhash,
                "💾 State dowload is complete ({}/{})",
                self.received_coins,
                self.total_coins,
            );

            self.snapshot_manager.create_snapshot(self.total_coins);

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
