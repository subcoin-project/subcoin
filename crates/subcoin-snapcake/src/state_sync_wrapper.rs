use crate::snapshot_manager::{SnapshotManager, StateSyncCursor};
use codec::Decode;
use sc_client_api::ProofProvider;
use sc_network_sync::strategy::state_sync::{
    ImportResult, StateSync, StateSyncProgress, StateSyncProvider,
};
use sc_network_sync::{StateRequest, StateResponse};
use smallvec::SmallVec;
use sp_runtime::SaturatedConversion;
use sp_runtime::traits::{Block as BlockT, Header as HeaderT, NumberFor};
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

    /// Creates a new instance resuming from a previous sync position.
    ///
    /// This constructor is used when a cursor file exists, allowing the sync
    /// to continue from where it left off.
    pub(crate) fn new_resuming(
        client: Arc<Client>,
        target_header: B::Header,
        skip_proof: bool,
        snapshot_base_dir: PathBuf,
        total_coins: usize,
        cursor: StateSyncCursor,
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

        // Convert Vec<Vec<u8>> to SmallVec<[Vec<u8>; 2]>
        let last_key: SmallVec<[Vec<u8>; 2]> = cursor.last_key.into();

        tracing::info!(
            target: "snapcake",
            "Resuming state sync from position, received_coins: {}/{}",
            cursor.received_coins,
            total_coins,
        );

        Self {
            inner: StateSync::new_with_last_key(
                client,
                target_header,
                None,
                None,
                skip_proof,
                last_key,
            ),
            target_block_number: target_block_number.saturated_into(),
            target_bitcoin_block_hash,
            muhash: MuHash3072::new(),
            snapshot_manager,
            received_coins: cursor.received_coins as usize,
            total_coins,
            last_progress_print_time: None,
        }
    }

    /// Saves the current sync progress to disk.
    fn save_cursor(&self) {
        let last_key = self.inner.next_request().start;

        let cursor = StateSyncCursor {
            target_block_number: self.target_block_number,
            target_bitcoin_block_hash: self.target_bitcoin_block_hash,
            last_key,
            total_coins: self.total_coins as u64,
            received_coins: self.received_coins as u64,
        };

        if let Err(e) = self.snapshot_manager.save_cursor(&cursor) {
            tracing::warn!(
                target: "snapcake",
                "Failed to save state sync cursor: {e}"
            );
        }
    }

    // Find the Coin storage key values and store them locally.
    fn process_state_response(&mut self, response: &StateResponse) {
        let mut complete = false;

        // Count total entries for pre-allocation
        let total_entries: usize = response
            .entries
            .iter()
            .map(|entry| entry.entries.len())
            .sum();

        let key_values = response.entries.iter().flat_map(|key_value_state_entry| {
            if key_value_state_entry.complete {
                complete = true;
            }

            key_value_state_entry
                .entries
                .iter()
                .map(|state_entry| (&state_entry.key, &state_entry.value))
        });

        // Pre-allocate with capacity (upper bound, not all entries are UTXOs)
        let mut utxos_batch = Vec::with_capacity(total_entries);

        for (key, value) in key_values {
            if key.len() > 32 {
                // Attempt to decode the Coin storage item.
                if let Ok((txid, vout)) =
                    <(pallet_bitcoin::types::Txid, u32)>::decode(&mut &key.as_slice()[32..])
                {
                    let txid: bitcoin::Txid = txid.into();

                    let coin = Coin::decode(&mut value.as_slice())
                        .expect("Coin in state response must be decoded successfully; qed");

                    let data =
                        subcoin_utxo_snapshot::tx_out_ser(bitcoin::OutPoint { txid, vout }, &coin)
                            .expect("Failed to serialize txout");

                    self.muhash.insert(&data);

                    utxos_batch.push(Utxo { txid, vout, coin });
                }
            }
        }

        // Batch write all UTXOs from this response
        if !utxos_batch.is_empty() {
            let batch_size = self.snapshot_manager.store_utxos_batch(&utxos_batch);
            self.received_coins += batch_size;
        }

        if self
            .last_progress_print_time
            .is_none_or(|last_time| last_time.elapsed() > DOWNLOAD_PROGRESS_LOG_INTERVAL)
        {
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

            // Clear the cursor since state sync is complete
            if let Err(e) = self.snapshot_manager.clear_cursor() {
                tracing::warn!(
                    target: "snapcake",
                    "Failed to clear state sync cursor: {e}"
                );
            }

            self.snapshot_manager
                .create_snapshot(self.total_coins)
                .expect("Failed to create snapshot");

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
        let result = self.inner.import(response);

        // Save cursor after each successful chunk (but not on completion)
        if matches!(result, ImportResult::Continue) {
            self.save_cursor();
        }

        result
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
