//! Bitcoin state storage implementation using RocksDB with MuHash commitment.

use crate::undo::BlockUndo;
use crate::{cf, meta_keys, Coin, Error, Result};
use bitcoin::hashes::Hash;
use bitcoin::{Block, OutPoint, Transaction};
use parking_lot::RwLock;
use rocksdb::{ColumnFamily, ColumnFamilyDescriptor, Options, WriteBatch, DB};
use std::collections::HashMap;
use std::path::Path;
use subcoin_crypto::MuHash3072;

/// Convert OutPoint to storage key (36 bytes).
///
/// Format: txid (32 bytes, raw) || vout (4 bytes, little-endian)
fn outpoint_to_key(outpoint: &OutPoint) -> [u8; 36] {
    let mut key = [0u8; 36];
    key[..32].copy_from_slice(outpoint.txid.as_ref());
    key[32..].copy_from_slice(&outpoint.vout.to_le_bytes());
    key
}

/// Parse storage key back to OutPoint.
fn key_to_outpoint(key: &[u8; 36]) -> OutPoint {
    let mut txid_bytes = [0u8; 32];
    txid_bytes.copy_from_slice(&key[..32]);
    let txid = bitcoin::Txid::from_byte_array(txid_bytes);
    let vout = u32::from_le_bytes(key[32..].try_into().unwrap());
    OutPoint { txid, vout }
}

/// Bitcoin state (UTXO set) with MuHash commitment tracking.
///
/// This provides O(1) UTXO operations and O(1) MuHash updates per UTXO change,
/// compared to O(log n) for Substrate's Merkle Patricia Trie.
pub struct BitcoinState {
    /// RocksDB instance.
    db: DB,
    /// Rolling MuHash accumulator (protected by RwLock for concurrent reads).
    muhash: RwLock<MuHash3072>,
    /// Total UTXO count.
    utxo_count: RwLock<u64>,
    /// Current block height.
    height: RwLock<u32>,
}

impl BitcoinState {
    /// Open or create UTXO storage at the given path.
    pub fn open(path: &Path) -> Result<Self> {
        let mut db_opts = Options::default();
        db_opts.create_if_missing(true);
        db_opts.create_missing_column_families(true);

        // Optimize for UTXO workload
        db_opts.set_write_buffer_size(256 * 1024 * 1024); // 256MB write buffer
        db_opts.set_max_write_buffer_number(4);
        db_opts.set_target_file_size_base(256 * 1024 * 1024);
        db_opts.set_compression_type(rocksdb::DBCompressionType::Lz4);

        // Enable bloom filters for faster lookups
        let mut block_opts = rocksdb::BlockBasedOptions::default();
        block_opts.set_bloom_filter(10.0, false);
        db_opts.set_block_based_table_factory(&block_opts);

        // Define column families
        let cf_descriptors = vec![
            ColumnFamilyDescriptor::new(cf::UTXOS, Options::default()),
            ColumnFamilyDescriptor::new(cf::UNDO, Options::default()),
            ColumnFamilyDescriptor::new(cf::META, Options::default()),
        ];

        let db = DB::open_cf_descriptors(&db_opts, path, cf_descriptors)?;

        // Load persisted state
        let muhash = Self::load_muhash(&db)?;
        let utxo_count = Self::load_utxo_count(&db)?;
        let height = Self::load_height(&db)?;

        tracing::info!(
            "Opened Bitcoin state at height {height}, {utxo_count} UTXOs, muhash: {}",
            muhash.txoutset_muhash()
        );

        Ok(Self {
            db,
            muhash: RwLock::new(muhash),
            utxo_count: RwLock::new(utxo_count),
            height: RwLock::new(height),
        })
    }

    /// Create a new in-memory storage for testing.
    #[cfg(test)]
    pub fn open_temp() -> Result<Self> {
        let temp_dir = tempfile::tempdir().map_err(Error::Io)?;
        Self::open(temp_dir.path())
    }

    /// Get a UTXO by outpoint.
    pub fn get(&self, outpoint: &OutPoint) -> Option<Coin> {
        let cf = self.db.cf_handle(cf::UTXOS)?;
        let key = outpoint_to_key(outpoint);

        self.db
            .get_cf(cf, key)
            .ok()
            .flatten()
            .and_then(|bytes| Coin::decode_from_storage(&bytes).ok())
    }

    /// Check if a UTXO exists.
    pub fn contains(&self, outpoint: &OutPoint) -> bool {
        let Some(cf) = self.db.cf_handle(cf::UTXOS) else {
            return false;
        };
        let key = outpoint_to_key(outpoint);
        self.db.get_cf(cf, key).ok().flatten().is_some()
    }

    /// Apply a Bitcoin block's UTXO changes.
    ///
    /// This updates the UTXO set, MuHash, and saves undo data for potential reorgs.
    pub fn apply_block(&self, block: &Block, height: u32) -> Result<()> {
        let cf_utxos = self.db.cf_handle(cf::UTXOS).ok_or(Error::NotInitialized)?;
        let cf_undo = self.db.cf_handle(cf::UNDO).ok_or(Error::NotInitialized)?;
        let cf_meta = self.db.cf_handle(cf::META).ok_or(Error::NotInitialized)?;

        let mut batch = WriteBatch::default();
        let mut undo = BlockUndo::new();
        let mut muhash = self.muhash.write();
        let mut created: u64 = 0;
        let mut spent: u64 = 0;

        // Track UTXOs created within this block (not yet in DB)
        // This handles the case where a later tx spends an output from an earlier tx in the same block
        let mut in_block_utxos: HashMap<OutPoint, Coin> = HashMap::new();

        for tx in &block.txdata {
            self.process_transaction(
                tx,
                height,
                cf_utxos,
                &mut batch,
                &mut undo,
                &mut muhash,
                &mut created,
                &mut spent,
                &mut in_block_utxos,
            )?;
        }

        // Update metadata
        let new_utxo_count = {
            let mut count = self.utxo_count.write();
            *count = count
                .checked_add(created)
                .and_then(|c| c.checked_sub(spent))
                .ok_or_else(|| Error::InvalidHeight("UTXO count underflow".to_string()))?;
            *count
        };

        // Save undo data
        let undo_key = height.to_be_bytes();
        batch.put_cf(cf_undo, undo_key, undo.encode());

        // Save metadata
        batch.put_cf(cf_meta, meta_keys::HEIGHT, height.to_le_bytes());
        batch.put_cf(cf_meta, meta_keys::UTXO_COUNT, new_utxo_count.to_le_bytes());
        batch.put_cf(cf_meta, meta_keys::MUHASH, muhash.serialize());

        // Atomic write
        self.db.write(batch)?;

        // Update height
        *self.height.write() = height;

        tracing::debug!(
            "Applied block {height}: +{created} -{spent} UTXOs, total: {new_utxo_count}"
        );

        Ok(())
    }

    /// Process a single transaction's UTXO changes.
    #[allow(clippy::too_many_arguments)]
    fn process_transaction(
        &self,
        tx: &Transaction,
        height: u32,
        cf_utxos: &ColumnFamily,
        batch: &mut WriteBatch,
        undo: &mut BlockUndo,
        muhash: &mut MuHash3072,
        created: &mut u64,
        spent: &mut u64,
        in_block_utxos: &mut std::collections::HashMap<OutPoint, Coin>,
    ) -> Result<()> {
        let txid = tx.compute_txid();
        let is_coinbase = tx.is_coinbase();

        // Process inputs (spend UTXOs) - skip for coinbase
        if !is_coinbase {
            for input in &tx.input {
                let outpoint = input.previous_output;
                let key = outpoint_to_key(&outpoint);

                // First check in-block UTXOs (created by earlier tx in same block)
                // Then check the database
                let coin = if let Some(coin) = in_block_utxos.remove(&outpoint) {
                    // Found in current block - already pending in batch, just remove from tracking
                    coin
                } else {
                    // Look up in database
                    let coin_bytes = self
                        .db
                        .get_cf(cf_utxos, key)?
                        .ok_or(Error::UtxoNotFound(outpoint))?;

                    Coin::decode_from_storage(&coin_bytes)?
                };

                // Record for undo
                undo.record_spend(outpoint, coin.clone());

                // Remove from MuHash
                let muhash_data = coin.serialize_for_muhash(&outpoint);
                muhash.remove(&muhash_data);

                // Remove from storage (batch delete - handles both DB and pending writes)
                batch.delete_cf(cf_utxos, key);
                *spent += 1;
            }
        }

        // Process outputs (create UTXOs)
        for (vout, output) in tx.output.iter().enumerate() {
            // Skip OP_RETURN outputs (unspendable)
            if output.script_pubkey.is_op_return() {
                continue;
            }

            let outpoint = OutPoint {
                txid,
                vout: vout as u32,
            };
            let coin = Coin::from_txout(output, height, is_coinbase);
            let key = outpoint_to_key(&outpoint);

            // Check for duplicate coinbase txid (BIP30 edge case).
            // Bitcoin had duplicate coinbase txids in blocks 91722/91812 and 91880/91842.
            // If the outpoint already exists, we need to handle it specially:
            // - Remove old entry from MuHash before adding new
            // - Don't increment count (we're replacing, not creating)
            let is_duplicate = self.db.get_cf(cf_utxos, key).ok().flatten().is_some();

            if is_duplicate {
                // Remove old coin from MuHash (we'll add the new one below)
                if let Some(old_bytes) = self.db.get_cf(cf_utxos, key).ok().flatten() {
                    if let Ok(old_coin) = Coin::decode_from_storage(&old_bytes) {
                        let old_muhash_data = old_coin.serialize_for_muhash(&outpoint);
                        muhash.remove(&old_muhash_data);
                    }
                }
                tracing::warn!(
                    "Duplicate coinbase txid at height {height}: {outpoint:?} (BIP30 edge case)"
                );
            }

            // Record for undo
            undo.record_create(outpoint);

            // Add to MuHash
            let muhash_data = coin.serialize_for_muhash(&outpoint);
            muhash.insert(&muhash_data);

            // Track in-block UTXOs so later txs in this block can spend them
            in_block_utxos.insert(outpoint, coin.clone());

            // Add to storage
            batch.put_cf(cf_utxos, key, coin.encode_for_storage());

            // Only increment count if this is a NEW entry, not a duplicate
            if !is_duplicate {
                *created += 1;
            }
        }

        Ok(())
    }

    /// Revert a block using its undo data.
    ///
    /// This is used during chain reorganizations.
    pub fn revert_block(&self, height: u32) -> Result<()> {
        let cf_utxos = self.db.cf_handle(cf::UTXOS).ok_or(Error::NotInitialized)?;
        let cf_undo = self.db.cf_handle(cf::UNDO).ok_or(Error::NotInitialized)?;
        let cf_meta = self.db.cf_handle(cf::META).ok_or(Error::NotInitialized)?;

        // Load undo data
        let undo_key = height.to_be_bytes();
        let undo_bytes = self
            .db
            .get_cf(cf_undo, undo_key)?
            .ok_or(Error::UndoNotFound(height))?;

        let undo = BlockUndo::decode(&undo_bytes)?;

        let mut batch = WriteBatch::default();
        let mut muhash = self.muhash.write();

        // Remove created UTXOs
        for outpoint in &undo.created_outpoints {
            let key = outpoint_to_key(outpoint);

            // Get the coin to remove from MuHash
            if let Some(coin_bytes) = self.db.get_cf(&cf_utxos, key)? {
                let coin = Coin::decode_from_storage(&coin_bytes)?;
                let muhash_data = coin.serialize_for_muhash(outpoint);
                muhash.remove(&muhash_data);
            }

            batch.delete_cf(&cf_utxos, key);
        }

        // Restore spent UTXOs
        for (outpoint, coin) in &undo.spent_utxos {
            let key = outpoint_to_key(outpoint);

            // Add back to MuHash
            let muhash_data = coin.serialize_for_muhash(outpoint);
            muhash.insert(&muhash_data);

            // Add back to storage
            batch.put_cf(&cf_utxos, key, coin.encode_for_storage());
        }

        // Update metadata
        let new_height = height.saturating_sub(1);
        let new_utxo_count = {
            let mut count = self.utxo_count.write();
            *count = count
                .checked_sub(undo.created_count() as u64)
                .and_then(|c| c.checked_add(undo.spent_count() as u64))
                .ok_or_else(|| Error::InvalidHeight("UTXO count overflow on revert".to_string()))?;
            *count
        };

        batch.put_cf(cf_meta, meta_keys::HEIGHT, new_height.to_le_bytes());
        batch.put_cf(cf_meta, meta_keys::UTXO_COUNT, new_utxo_count.to_le_bytes());
        batch.put_cf(cf_meta, meta_keys::MUHASH, muhash.serialize());

        // Delete the undo data we just used
        batch.delete_cf(cf_undo, undo_key);

        // Atomic write
        self.db.write(batch)?;

        // Update height
        *self.height.write() = new_height;

        tracing::info!(
            "Reverted block {height}: -{} +{} UTXOs, now at height {new_height}",
            undo.created_count(),
            undo.spent_count()
        );

        Ok(())
    }

    /// Get the current UTXO set commitment (32 bytes).
    pub fn commitment(&self) -> [u8; 32] {
        let muhash = self.muhash.read();
        let digest = muhash.digest();
        digest.try_into().expect("MuHash digest should be 32 bytes")
    }

    /// Get the MuHash in Bitcoin Core's hex format for verification.
    pub fn muhash_hex(&self) -> String {
        self.muhash.read().txoutset_muhash()
    }

    /// Get the current block height of the UTXO set.
    pub fn height(&self) -> u32 {
        *self.height.read()
    }

    /// Get the current UTXO count.
    pub fn utxo_count(&self) -> u64 {
        *self.utxo_count.read()
    }

    /// Get the serialized MuHash (768 bytes) for P2P transmission.
    pub fn muhash_serialized(&self) -> [u8; 768] {
        self.muhash.read().serialize()
    }

    /// Verify MuHash against expected checkpoint value.
    ///
    /// Returns `true` if the current MuHash matches the expected hex string.
    pub fn verify_muhash(&self, expected_hex: &str) -> bool {
        self.muhash_hex() == expected_hex
    }

    // --- UTXO Sync Methods (for P2P fast sync) ---

    /// Export a chunk of UTXOs for serving to peers during fast sync.
    ///
    /// Returns UTXOs in lexicographic order by OutPoint for deterministic pagination.
    ///
    /// # Arguments
    /// * `cursor` - Starting point for iteration (exclusive). None starts from beginning.
    /// * `max_entries` - Maximum number of UTXOs to return.
    ///
    /// # Returns
    /// * Vector of (OutPoint, Coin) pairs
    /// * Next cursor for pagination (None if no more entries)
    /// * Boolean indicating if this is the last chunk
    pub fn export_chunk(
        &self,
        cursor: Option<&[u8; 36]>,
        max_entries: u32,
    ) -> Result<crate::ExportChunkResult> {
        let cf = self.db.cf_handle(cf::UTXOS).ok_or(Error::NotInitialized)?;

        let mut entries = Vec::with_capacity(max_entries as usize);
        let mut iter = self.db.raw_iterator_cf(cf);

        // Position iterator
        match cursor {
            Some(start_key) => {
                iter.seek(start_key);
                // Skip the cursor key itself (exclusive start)
                if iter.valid() && iter.key() == Some(start_key.as_slice()) {
                    iter.next();
                }
            }
            None => iter.seek_to_first(),
        }

        let mut last_key: Option<[u8; 36]> = None;

        while iter.valid() && entries.len() < max_entries as usize {
            if let (Some(key), Some(value)) = (iter.key(), iter.value()) {
                if key.len() == 36 {
                    let key_array: [u8; 36] = key.try_into().unwrap();
                    let outpoint = key_to_outpoint(&key_array);
                    let coin = Coin::decode_from_storage(value)?;
                    entries.push((outpoint, coin));
                    last_key = Some(key_array);
                }
            }
            iter.next();
        }

        let is_complete = !iter.valid();
        let next_cursor = if is_complete { None } else { last_key };

        Ok((entries, next_cursor, is_complete))
    }

    /// Count actual entries in the UTXOS column family (for debugging).
    ///
    /// This iterates through all entries and counts them, comparing with metadata.
    pub fn count_actual_utxos(&self) -> Result<(u64, u64, u64)> {
        let cf = self.db.cf_handle(cf::UTXOS).ok_or(Error::NotInitialized)?;

        let mut iter = self.db.raw_iterator_cf(cf);
        iter.seek_to_first();

        let mut total_entries = 0u64;
        let mut entries_36_bytes = 0u64;
        let mut entries_other = 0u64;

        while iter.valid() {
            if let Some(key) = iter.key() {
                total_entries += 1;
                if key.len() == 36 {
                    entries_36_bytes += 1;
                } else {
                    entries_other += 1;
                    tracing::warn!(
                        "Found entry with unexpected key length: {} bytes",
                        key.len()
                    );
                }
            }
            iter.next();
        }

        let metadata_count = self.utxo_count();

        tracing::info!(
            "UTXO count check: metadata={metadata_count}, actual_total={total_entries}, \
             36-byte keys={entries_36_bytes}, other keys={entries_other}"
        );

        Ok((metadata_count, entries_36_bytes, entries_other))
    }

    /// Verify and optionally repair MuHash consistency.
    ///
    /// Recomputes MuHash from actual DB contents and compares with stored value.
    /// If there's a mismatch, updates the stored MuHash and count to match reality.
    ///
    /// Returns (was_consistent, actual_count, actual_muhash_hex).
    pub fn verify_and_repair_muhash(&self) -> Result<(bool, u64, String)> {
        let cf_utxos = self.db.cf_handle(cf::UTXOS).ok_or(Error::NotInitialized)?;
        let cf_meta = self.db.cf_handle(cf::META).ok_or(Error::NotInitialized)?;

        // Recompute MuHash from actual DB contents
        let mut recomputed_muhash = MuHash3072::new();
        let mut actual_count: u64 = 0;

        let mut iter = self.db.raw_iterator_cf(cf_utxos);
        iter.seek_to_first();

        while iter.valid() {
            if let (Some(key), Some(value)) = (iter.key(), iter.value()) {
                if key.len() == 36 {
                    let key_array: [u8; 36] = key.try_into().unwrap();
                    let outpoint = key_to_outpoint(&key_array);
                    let coin = Coin::decode_from_storage(value)?;

                    let muhash_data = coin.serialize_for_muhash(&outpoint);
                    recomputed_muhash.insert(&muhash_data);
                    actual_count += 1;
                }
            }
            iter.next();
        }

        let stored_muhash_hex = self.muhash.read().txoutset_muhash();
        let recomputed_muhash_hex = recomputed_muhash.txoutset_muhash();
        let stored_count = *self.utxo_count.read();

        let is_consistent =
            stored_muhash_hex == recomputed_muhash_hex && stored_count == actual_count;

        if !is_consistent {
            tracing::warn!(
                "MuHash inconsistency detected! {stored_count}, {actual_count}, \
                 stored_muhash={stored_muhash_hex}, recomputed_muhash={recomputed_muhash_hex}"
            );

            // Repair: update stored values to match reality
            let mut batch = rocksdb::WriteBatch::default();
            batch.put_cf(cf_meta, meta_keys::MUHASH, recomputed_muhash.serialize());
            batch.put_cf(cf_meta, meta_keys::UTXO_COUNT, actual_count.to_le_bytes());
            self.db.write(batch)?;

            *self.muhash.write() = recomputed_muhash.clone();
            *self.utxo_count.write() = actual_count;

            tracing::info!("Repaired MuHash: count={actual_count}, muhash={recomputed_muhash_hex}");
        } else {
            tracing::info!(
                "MuHash verification passed: count={actual_count}, muhash={recomputed_muhash_hex}"
            );
        }

        Ok((is_consistent, actual_count, recomputed_muhash_hex))
    }

    /// Bulk import UTXOs for fast sync (no undo data).
    ///
    /// This is used during initial sync when downloading UTXO snapshots from peers.
    /// No undo data is stored since we trust the checkpoint verification.
    ///
    /// # Arguments
    /// * `entries` - Vector of (OutPoint, Coin) pairs to import
    ///
    /// # Returns
    /// Number of UTXOs successfully imported
    pub fn bulk_import(&self, entries: Vec<(OutPoint, Coin)>) -> Result<usize> {
        let cf_utxos = self.db.cf_handle(cf::UTXOS).ok_or(Error::NotInitialized)?;
        let cf_meta = self.db.cf_handle(cf::META).ok_or(Error::NotInitialized)?;

        let mut batch = WriteBatch::default();
        let mut muhash = self.muhash.write();
        let count = entries.len();

        for (outpoint, coin) in &entries {
            let key = outpoint_to_key(outpoint);

            // Add to MuHash
            let muhash_data = coin.serialize_for_muhash(outpoint);
            muhash.insert(&muhash_data);

            // Add to storage
            batch.put_cf(cf_utxos, key, coin.encode_for_storage());
        }

        // Update UTXO count
        let new_count = {
            let mut utxo_count = self.utxo_count.write();
            *utxo_count = utxo_count
                .checked_add(count as u64)
                .ok_or_else(|| Error::InvalidHeight("UTXO count overflow".to_string()))?;
            *utxo_count
        };

        // Save metadata
        batch.put_cf(cf_meta, meta_keys::UTXO_COUNT, new_count.to_le_bytes());
        batch.put_cf(cf_meta, meta_keys::MUHASH, muhash.serialize());

        // Atomic write
        self.db.write(batch)?;

        tracing::debug!("Bulk imported {count} UTXOs, total: {new_count}");

        Ok(count)
    }

    /// Finalize bulk import at a specific height.
    ///
    /// This is called after all UTXO chunks have been downloaded and verified.
    /// Sets the height and persists the final state.
    pub fn finalize_import(&self, height: u32) -> Result<()> {
        let cf_meta = self.db.cf_handle(cf::META).ok_or(Error::NotInitialized)?;

        let mut batch = WriteBatch::default();
        batch.put_cf(cf_meta, meta_keys::HEIGHT, height.to_le_bytes());

        // Also persist current muhash and count
        let muhash = self.muhash.read();
        let count = *self.utxo_count.read();
        batch.put_cf(cf_meta, meta_keys::MUHASH, muhash.serialize());
        batch.put_cf(cf_meta, meta_keys::UTXO_COUNT, count.to_le_bytes());

        self.db.write(batch)?;

        *self.height.write() = height;

        tracing::info!(
            "Finalized UTXO import at height {height}, {count} UTXOs, muhash: {}",
            muhash.txoutset_muhash()
        );

        Ok(())
    }

    /// Clear all UTXO data (for restarting fast sync after verification failure).
    ///
    /// This removes all UTXOs and resets metadata to initial state.
    pub fn clear(&self) -> Result<()> {
        let cf_utxos = self.db.cf_handle(cf::UTXOS).ok_or(Error::NotInitialized)?;
        let cf_undo = self.db.cf_handle(cf::UNDO).ok_or(Error::NotInitialized)?;
        let cf_meta = self.db.cf_handle(cf::META).ok_or(Error::NotInitialized)?;

        let mut batch = WriteBatch::default();

        // Delete all UTXOs
        let mut iter = self.db.raw_iterator_cf(cf_utxos);
        iter.seek_to_first();
        while iter.valid() {
            if let Some(key) = iter.key() {
                batch.delete_cf(cf_utxos, key);
            }
            iter.next();
        }

        // Delete all undo data
        let mut iter = self.db.raw_iterator_cf(cf_undo);
        iter.seek_to_first();
        while iter.valid() {
            if let Some(key) = iter.key() {
                batch.delete_cf(cf_undo, key);
            }
            iter.next();
        }

        // Reset metadata
        batch.put_cf(cf_meta, meta_keys::HEIGHT, 0u32.to_le_bytes());
        batch.put_cf(cf_meta, meta_keys::UTXO_COUNT, 0u64.to_le_bytes());
        batch.put_cf(cf_meta, meta_keys::MUHASH, MuHash3072::new().serialize());

        self.db.write(batch)?;

        // Reset in-memory state
        *self.height.write() = 0;
        *self.utxo_count.write() = 0;
        *self.muhash.write() = MuHash3072::new();

        tracing::info!("Cleared all UTXO data");

        Ok(())
    }

    /// Iterate over all UTXOs (for verification/debugging).
    ///
    /// Returns an iterator that yields (OutPoint, Coin) pairs in lexicographic order.
    pub fn iter_utxos(&self) -> UtxoIterator<'_> {
        UtxoIterator::new(&self.db)
    }

    // --- Private helper methods ---

    fn load_muhash(db: &DB) -> Result<MuHash3072> {
        let Some(cf) = db.cf_handle(cf::META) else {
            return Ok(MuHash3072::new());
        };

        match db.get_cf(cf, meta_keys::MUHASH)? {
            Some(bytes) if bytes.len() == 768 => {
                let arr: [u8; 768] = bytes.try_into().unwrap();
                Ok(MuHash3072::deserialize(&arr))
            }
            _ => Ok(MuHash3072::new()),
        }
    }

    fn load_utxo_count(db: &DB) -> Result<u64> {
        let Some(cf) = db.cf_handle(cf::META) else {
            return Ok(0);
        };

        match db.get_cf(cf, meta_keys::UTXO_COUNT)? {
            Some(bytes) if bytes.len() == 8 => Ok(u64::from_le_bytes(bytes.try_into().unwrap())),
            _ => Ok(0),
        }
    }

    fn load_height(db: &DB) -> Result<u32> {
        let Some(cf) = db.cf_handle(cf::META) else {
            return Ok(0);
        };

        match db.get_cf(cf, meta_keys::HEIGHT)? {
            Some(bytes) if bytes.len() == 4 => Ok(u32::from_le_bytes(bytes.try_into().unwrap())),
            _ => Ok(0),
        }
    }
}

/// Iterator over all UTXOs in the storage.
///
/// Yields (OutPoint, Coin) pairs in lexicographic order by OutPoint key.
pub struct UtxoIterator<'a> {
    iter: rocksdb::DBRawIterator<'a>,
}

impl<'a> UtxoIterator<'a> {
    fn new(db: &'a DB) -> Self {
        let cf = db
            .cf_handle(cf::UTXOS)
            .expect("UTXOS column family must exist");
        let mut iter = db.raw_iterator_cf(cf);
        iter.seek_to_first();
        Self { iter }
    }
}

impl Iterator for UtxoIterator<'_> {
    type Item = (OutPoint, Coin);

    fn next(&mut self) -> Option<Self::Item> {
        while self.iter.valid() {
            if let (Some(key), Some(value)) = (self.iter.key(), self.iter.value()) {
                if key.len() == 36 {
                    let key_array: [u8; 36] = key.try_into().ok()?;
                    let outpoint = key_to_outpoint(&key_array);
                    let coin = Coin::decode_from_storage(value).ok()?;
                    self.iter.next();
                    return Some((outpoint, coin));
                }
            }
            self.iter.next();
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::hashes::Hash;
    use bitcoin::{Amount, ScriptBuf, TxOut};

    fn create_test_block(height: u32, prev_outpoints: &[OutPoint]) -> Block {
        use bitcoin::blockdata::block::{Header, Version};
        use bitcoin::blockdata::transaction::{Transaction, TxIn, Version as TxVersion};
        use bitcoin::CompactTarget;

        // Create unique coinbase script_sig with height (like real Bitcoin blocks)
        let mut coinbase_script = vec![0x03]; // Push 3 bytes
        coinbase_script.extend_from_slice(&height.to_le_bytes()[..3]);

        let coinbase_tx = Transaction {
            version: TxVersion::TWO,
            lock_time: bitcoin::absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::null(),
                script_sig: ScriptBuf::from_bytes(coinbase_script),
                sequence: bitcoin::Sequence::MAX,
                witness: bitcoin::Witness::new(),
            }],
            output: vec![TxOut {
                value: Amount::from_sat(5_000_000_000),
                script_pubkey: ScriptBuf::new_p2pkh(&bitcoin::PubkeyHash::all_zeros()),
            }],
        };

        let mut txs = vec![coinbase_tx];

        // Add a spending transaction if we have previous outpoints
        if !prev_outpoints.is_empty() {
            let spending_tx = Transaction {
                version: TxVersion::TWO,
                lock_time: bitcoin::absolute::LockTime::ZERO,
                input: prev_outpoints
                    .iter()
                    .map(|op| TxIn {
                        previous_output: *op,
                        script_sig: ScriptBuf::new(),
                        sequence: bitcoin::Sequence::MAX,
                        witness: bitcoin::Witness::new(),
                    })
                    .collect(),
                output: vec![TxOut {
                    value: Amount::from_sat(1_000_000),
                    script_pubkey: ScriptBuf::new_p2pkh(&bitcoin::PubkeyHash::all_zeros()),
                }],
            };
            txs.push(spending_tx);
        }

        Block {
            header: Header {
                version: Version::TWO,
                prev_blockhash: bitcoin::BlockHash::all_zeros(),
                merkle_root: bitcoin::TxMerkleNode::all_zeros(),
                time: 0,
                bits: CompactTarget::from_consensus(0),
                nonce: height,
            },
            txdata: txs,
        }
    }

    #[test]
    fn test_apply_genesis_block() {
        let storage = BitcoinState::open_temp().unwrap();

        let block = create_test_block(0, &[]);
        storage.apply_block(&block, 0).unwrap();

        assert_eq!(storage.height(), 0);
        assert_eq!(storage.utxo_count(), 1); // One coinbase output

        // Verify the coinbase UTXO exists
        let coinbase_outpoint = OutPoint {
            txid: block.txdata[0].compute_txid(),
            vout: 0,
        };
        assert!(storage.contains(&coinbase_outpoint));

        let coin = storage.get(&coinbase_outpoint).unwrap();
        assert!(coin.is_coinbase);
        assert_eq!(coin.amount, 5_000_000_000);
        assert_eq!(coin.height, 0);
    }

    #[test]
    fn test_apply_and_revert_block() {
        let storage = BitcoinState::open_temp().unwrap();

        // Apply genesis block
        let block0 = create_test_block(0, &[]);
        storage.apply_block(&block0, 0).unwrap();

        let coinbase_outpoint = OutPoint {
            txid: block0.txdata[0].compute_txid(),
            vout: 0,
        };

        // Save state after block 0
        let muhash_after_0 = storage.muhash_hex();
        let count_after_0 = storage.utxo_count();

        // Apply block 1 that spends the coinbase
        let block1 = create_test_block(1, &[coinbase_outpoint]);
        storage.apply_block(&block1, 1).unwrap();

        assert_eq!(storage.height(), 1);
        // Coinbase spent, new coinbase + spending tx output created
        assert_eq!(storage.utxo_count(), 2);
        // Original coinbase should be gone
        assert!(!storage.contains(&coinbase_outpoint));

        // Revert block 1
        storage.revert_block(1).unwrap();

        assert_eq!(storage.height(), 0);
        assert_eq!(storage.utxo_count(), count_after_0);
        assert_eq!(storage.muhash_hex(), muhash_after_0);
        // Original coinbase should be restored
        assert!(storage.contains(&coinbase_outpoint));
    }

    #[test]
    fn test_muhash_consistency() {
        let storage = BitcoinState::open_temp().unwrap();

        // Apply multiple blocks
        let block0 = create_test_block(0, &[]);
        storage.apply_block(&block0, 0).unwrap();

        let coinbase0 = OutPoint {
            txid: block0.txdata[0].compute_txid(),
            vout: 0,
        };

        let block1 = create_test_block(1, &[coinbase0]);
        storage.apply_block(&block1, 1).unwrap();

        let muhash_at_1 = storage.muhash_hex();

        // Revert and re-apply should give same muhash
        storage.revert_block(1).unwrap();
        storage.apply_block(&block1, 1).unwrap();

        assert_eq!(storage.muhash_hex(), muhash_at_1);
    }

    /// Test in-block UTXO spending: a later transaction spends an output created
    /// by an earlier transaction in the same block.
    #[test]
    fn test_in_block_utxo_spending() {
        use bitcoin::blockdata::block::{Header, Version};
        use bitcoin::blockdata::transaction::{Transaction, TxIn, Version as TxVersion};
        use bitcoin::CompactTarget;

        let storage = BitcoinState::open_temp().unwrap();

        // Create a block with 3 transactions:
        // tx0: coinbase -> output A
        // tx1: (from prev block) -> output B
        // tx2: spends output B (created by tx1 in the same block!)

        // First, apply a genesis block to have something to spend
        let genesis = create_test_block(0, &[]);
        storage.apply_block(&genesis, 0).unwrap();

        let genesis_coinbase = OutPoint {
            txid: genesis.txdata[0].compute_txid(),
            vout: 0,
        };

        // Build block 1 with in-block spending
        let coinbase_tx = Transaction {
            version: TxVersion::TWO,
            lock_time: bitcoin::absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::null(),
                script_sig: ScriptBuf::from_bytes(vec![0x01, 0x01]),
                sequence: bitcoin::Sequence::MAX,
                witness: bitcoin::Witness::new(),
            }],
            output: vec![TxOut {
                value: Amount::from_sat(5_000_000_000),
                script_pubkey: ScriptBuf::new_p2pkh(&bitcoin::PubkeyHash::all_zeros()),
            }],
        };

        // tx1: spends genesis coinbase, creates output B
        let tx1 = Transaction {
            version: TxVersion::TWO,
            lock_time: bitcoin::absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output: genesis_coinbase,
                script_sig: ScriptBuf::new(),
                sequence: bitcoin::Sequence::MAX,
                witness: bitcoin::Witness::new(),
            }],
            output: vec![TxOut {
                value: Amount::from_sat(4_000_000_000),
                script_pubkey: ScriptBuf::new_p2pkh(&bitcoin::PubkeyHash::all_zeros()),
            }],
        };

        let tx1_output = OutPoint {
            txid: tx1.compute_txid(),
            vout: 0,
        };

        // tx2: spends tx1's output (in-block spending!)
        let tx2 = Transaction {
            version: TxVersion::TWO,
            lock_time: bitcoin::absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output: tx1_output,
                script_sig: ScriptBuf::new(),
                sequence: bitcoin::Sequence::MAX,
                witness: bitcoin::Witness::new(),
            }],
            output: vec![TxOut {
                value: Amount::from_sat(3_000_000_000),
                script_pubkey: ScriptBuf::new_p2pkh(&bitcoin::PubkeyHash::all_zeros()),
            }],
        };

        let tx2_output = OutPoint {
            txid: tx2.compute_txid(),
            vout: 0,
        };

        let block1 = Block {
            header: Header {
                version: Version::TWO,
                prev_blockhash: bitcoin::BlockHash::all_zeros(),
                merkle_root: bitcoin::TxMerkleNode::all_zeros(),
                time: 0,
                bits: CompactTarget::from_consensus(0),
                nonce: 1,
            },
            txdata: vec![coinbase_tx, tx1, tx2],
        };

        // This should succeed - tx2 can spend tx1's output created in the same block
        storage.apply_block(&block1, 1).unwrap();

        assert_eq!(storage.height(), 1);
        // genesis coinbase spent, tx1 output spent (in-block), coinbase output + tx2 output created
        // Net: 2 UTXOs (coinbase from block1, tx2 output)
        assert_eq!(storage.utxo_count(), 2);

        // tx1's output should NOT exist (it was spent in-block)
        assert!(!storage.contains(&tx1_output));

        // tx2's output should exist
        assert!(storage.contains(&tx2_output));

        // CRITICAL: Verify count consistency - metadata should match actual DB
        let (metadata_count, actual_count, other_keys) = storage.count_actual_utxos().unwrap();
        assert_eq!(
            metadata_count, actual_count,
            "Metadata count ({metadata_count}) should match actual DB entries ({actual_count})"
        );
        assert_eq!(other_keys, 0, "Should not have any non-36-byte keys");

        // Verify MuHash consistency
        let (is_consistent, _, _) = storage.verify_and_repair_muhash().unwrap();
        assert!(
            is_consistent,
            "MuHash should be consistent without needing repair"
        );
    }

    /// Test count consistency after multiple blocks with in-block spending.
    #[test]
    fn test_count_consistency_with_in_block_spending() {
        use bitcoin::blockdata::block::{Header, Version};
        use bitcoin::blockdata::transaction::{Transaction, TxIn, Version as TxVersion};
        use bitcoin::CompactTarget;

        let storage = BitcoinState::open_temp().unwrap();

        // Apply genesis block
        let genesis = create_test_block(0, &[]);
        storage.apply_block(&genesis, 0).unwrap();

        let mut prev_coinbase = OutPoint {
            txid: genesis.txdata[0].compute_txid(),
            vout: 0,
        };

        // Apply 10 blocks, each with in-block spending
        for height in 1..=10 {
            let coinbase_tx = Transaction {
                version: TxVersion::TWO,
                lock_time: bitcoin::absolute::LockTime::ZERO,
                input: vec![TxIn {
                    previous_output: OutPoint::null(),
                    script_sig: ScriptBuf::from_bytes(vec![height as u8, 0x01]),
                    sequence: bitcoin::Sequence::MAX,
                    witness: bitcoin::Witness::new(),
                }],
                output: vec![TxOut {
                    value: Amount::from_sat(5_000_000_000),
                    script_pubkey: ScriptBuf::new_p2pkh(&bitcoin::PubkeyHash::all_zeros()),
                }],
            };

            // tx1: spends previous coinbase, creates output
            let tx1 = Transaction {
                version: TxVersion::TWO,
                lock_time: bitcoin::absolute::LockTime::ZERO,
                input: vec![TxIn {
                    previous_output: prev_coinbase,
                    script_sig: ScriptBuf::new(),
                    sequence: bitcoin::Sequence::MAX,
                    witness: bitcoin::Witness::new(),
                }],
                output: vec![TxOut {
                    value: Amount::from_sat(4_000_000_000),
                    script_pubkey: ScriptBuf::new_p2pkh(&bitcoin::PubkeyHash::all_zeros()),
                }],
            };

            let tx1_output = OutPoint {
                txid: tx1.compute_txid(),
                vout: 0,
            };

            // tx2: spends tx1's output (in-block spending!)
            let tx2 = Transaction {
                version: TxVersion::TWO,
                lock_time: bitcoin::absolute::LockTime::ZERO,
                input: vec![TxIn {
                    previous_output: tx1_output,
                    script_sig: ScriptBuf::new(),
                    sequence: bitcoin::Sequence::MAX,
                    witness: bitcoin::Witness::new(),
                }],
                output: vec![TxOut {
                    value: Amount::from_sat(3_000_000_000),
                    script_pubkey: ScriptBuf::new_p2pkh(&bitcoin::PubkeyHash::all_zeros()),
                }],
            };

            let block = Block {
                header: Header {
                    version: Version::TWO,
                    prev_blockhash: bitcoin::BlockHash::all_zeros(),
                    merkle_root: bitcoin::TxMerkleNode::all_zeros(),
                    time: 0,
                    bits: CompactTarget::from_consensus(0),
                    nonce: height,
                },
                txdata: vec![coinbase_tx.clone(), tx1, tx2],
            };

            storage.apply_block(&block, height).unwrap();

            // Update for next iteration
            prev_coinbase = OutPoint {
                txid: coinbase_tx.compute_txid(),
                vout: 0,
            };

            // Check count consistency after each block
            let (metadata_count, actual_count, _) = storage.count_actual_utxos().unwrap();
            assert_eq!(
                metadata_count, actual_count,
                "Count mismatch at height {height}: metadata={metadata_count}, actual={actual_count}"
            );
        }

        // Final MuHash consistency check
        let (is_consistent, actual_count, _) = storage.verify_and_repair_muhash().unwrap();
        assert!(is_consistent, "MuHash should be consistent");
        assert_eq!(storage.utxo_count(), actual_count);
    }

    /// Test for the duplicate coinbase bug (BIP30 violation in early Bitcoin blocks).
    ///
    /// Bitcoin had blocks with duplicate coinbase txids:
    /// - Block 91722 and 91812 share the same coinbase txid
    /// - Block 91880 and 91842 share the same coinbase txid
    ///
    /// This test verifies that we handle this case correctly.
    #[test]
    fn test_duplicate_coinbase_txid_handling() {
        use bitcoin::blockdata::block::{Header, Version};
        use bitcoin::blockdata::transaction::{Transaction, TxIn, Version as TxVersion};
        use bitcoin::CompactTarget;

        let storage = BitcoinState::open_temp().unwrap();

        // Create a coinbase transaction that will be used by two blocks
        let duplicate_coinbase = Transaction {
            version: TxVersion::TWO,
            lock_time: bitcoin::absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::null(),
                // Same script to get same txid
                script_sig: ScriptBuf::from_bytes(vec![0x04, 0xff, 0xff, 0x00, 0x1d, 0x01, 0x04]),
                sequence: bitcoin::Sequence::MAX,
                witness: bitcoin::Witness::new(),
            }],
            output: vec![TxOut {
                value: Amount::from_sat(5_000_000_000),
                script_pubkey: ScriptBuf::new_p2pkh(&bitcoin::PubkeyHash::all_zeros()),
            }],
        };

        let duplicate_outpoint = OutPoint {
            txid: duplicate_coinbase.compute_txid(),
            vout: 0,
        };

        // Block 1: First block with this coinbase
        let block1 = Block {
            header: Header {
                version: Version::TWO,
                prev_blockhash: bitcoin::BlockHash::all_zeros(),
                merkle_root: bitcoin::TxMerkleNode::all_zeros(),
                time: 0,
                bits: CompactTarget::from_consensus(0),
                nonce: 1,
            },
            txdata: vec![duplicate_coinbase.clone()],
        };

        storage.apply_block(&block1, 1).unwrap();
        assert_eq!(storage.utxo_count(), 1);
        assert!(storage.contains(&duplicate_outpoint));

        // Block 2: Different unique coinbase
        let unique_coinbase = Transaction {
            version: TxVersion::TWO,
            lock_time: bitcoin::absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::null(),
                script_sig: ScriptBuf::from_bytes(vec![0x02, 0x02]),
                sequence: bitcoin::Sequence::MAX,
                witness: bitcoin::Witness::new(),
            }],
            output: vec![TxOut {
                value: Amount::from_sat(5_000_000_000),
                script_pubkey: ScriptBuf::new_p2pkh(&bitcoin::PubkeyHash::all_zeros()),
            }],
        };

        let block2 = Block {
            header: Header {
                version: Version::TWO,
                prev_blockhash: bitcoin::BlockHash::all_zeros(),
                merkle_root: bitcoin::TxMerkleNode::all_zeros(),
                time: 0,
                bits: CompactTarget::from_consensus(0),
                nonce: 2,
            },
            txdata: vec![unique_coinbase],
        };

        storage.apply_block(&block2, 2).unwrap();
        assert_eq!(storage.utxo_count(), 2);

        // Block 3: DUPLICATE coinbase (same txid as block 1!)
        // This simulates the BIP30 bug in blocks 91722/91812 and 91880/91842
        let block3 = Block {
            header: Header {
                version: Version::TWO,
                prev_blockhash: bitcoin::BlockHash::all_zeros(),
                merkle_root: bitcoin::TxMerkleNode::all_zeros(),
                time: 0,
                bits: CompactTarget::from_consensus(0),
                nonce: 3,
            },
            txdata: vec![duplicate_coinbase],
        };

        storage.apply_block(&block3, 3).unwrap();

        // BUG: Without fix, count would be 3 but actual DB has only 2 entries!
        // The duplicate overwrites the existing entry in DB but still increments count.

        // Verify count consistency
        let (metadata_count, actual_count, _) = storage.count_actual_utxos().unwrap();

        // This assertion will FAIL if the bug exists
        assert_eq!(
            metadata_count, actual_count,
            "Duplicate coinbase bug: metadata count ({metadata_count}) != actual DB entries ({actual_count})"
        );
    }

    // --- Tests for UTXO Sync Methods ---

    fn create_test_utxos(count: usize) -> Vec<(OutPoint, Coin)> {
        (0..count)
            .map(|i| {
                let txid = bitcoin::Txid::from_byte_array([i as u8; 32]);
                let outpoint = OutPoint { txid, vout: 0 };
                let coin = Coin {
                    is_coinbase: false,
                    amount: (i as u64 + 1) * 1000,
                    height: i as u32,
                    script_pubkey: vec![0x51], // OP_TRUE
                };
                (outpoint, coin)
            })
            .collect()
    }

    #[test]
    fn test_bulk_import_and_export() {
        let storage = BitcoinState::open_temp().unwrap();

        // Create test UTXOs
        let utxos = create_test_utxos(10);
        let original_muhash = storage.muhash_hex();

        // Bulk import
        let imported = storage.bulk_import(utxos.clone()).unwrap();
        assert_eq!(imported, 10);
        assert_eq!(storage.utxo_count(), 10);
        assert_ne!(storage.muhash_hex(), original_muhash);

        // Export all in one chunk
        let (exported, next_cursor, is_complete) = storage.export_chunk(None, 100).unwrap();
        assert_eq!(exported.len(), 10);
        assert!(is_complete);
        assert!(next_cursor.is_none());

        // Verify all UTXOs match
        for (outpoint, coin) in &utxos {
            let found = exported.iter().any(|(op, c)| op == outpoint && c == coin);
            assert!(found, "UTXO {outpoint:?} not found in export");
        }
    }

    #[test]
    fn test_export_chunk_pagination() {
        let storage = BitcoinState::open_temp().unwrap();

        // Import 25 UTXOs
        let utxos = create_test_utxos(25);
        storage.bulk_import(utxos).unwrap();

        // Export in chunks of 10
        let mut all_exported = Vec::new();
        let mut cursor: Option<[u8; 36]> = None;
        let mut chunks = 0;

        loop {
            let (chunk, next_cursor, is_complete) =
                storage.export_chunk(cursor.as_ref(), 10).unwrap();
            all_exported.extend(chunk);
            chunks += 1;

            if is_complete {
                break;
            }
            cursor = next_cursor;
        }

        assert_eq!(all_exported.len(), 25);
        assert_eq!(chunks, 3); // 10 + 10 + 5

        // Verify ordering is consistent (lexicographic by OutPoint key)
        for i in 1..all_exported.len() {
            let prev_key = outpoint_to_key(&all_exported[i - 1].0);
            let curr_key = outpoint_to_key(&all_exported[i].0);
            assert!(prev_key < curr_key, "UTXOs not in lexicographic order");
        }
    }

    #[test]
    fn test_bulk_import_muhash_consistency() {
        // Test that bulk_import produces the same MuHash as apply_block
        let storage1 = BitcoinState::open_temp().unwrap();
        let storage2 = BitcoinState::open_temp().unwrap();

        // Apply block to storage1
        let block = create_test_block(0, &[]);
        storage1.apply_block(&block, 0).unwrap();

        // Get the UTXO created by the block
        let coinbase_outpoint = OutPoint {
            txid: block.txdata[0].compute_txid(),
            vout: 0,
        };
        let coin = storage1.get(&coinbase_outpoint).unwrap();

        // Bulk import the same UTXO to storage2
        storage2
            .bulk_import(vec![(coinbase_outpoint, coin)])
            .unwrap();

        // MuHash should match
        assert_eq!(storage1.muhash_hex(), storage2.muhash_hex());
    }

    #[test]
    fn test_finalize_import() {
        let storage = BitcoinState::open_temp().unwrap();

        // Import UTXOs
        let utxos = create_test_utxos(5);
        storage.bulk_import(utxos).unwrap();
        assert_eq!(storage.height(), 0);

        // Finalize at height 100
        storage.finalize_import(100).unwrap();
        assert_eq!(storage.height(), 100);
        assert_eq!(storage.utxo_count(), 5);
    }

    #[test]
    fn test_clear() {
        let storage = BitcoinState::open_temp().unwrap();

        // Add some data
        let block = create_test_block(0, &[]);
        storage.apply_block(&block, 0).unwrap();
        assert_eq!(storage.height(), 0);
        assert_eq!(storage.utxo_count(), 1);

        let muhash_before_clear = storage.muhash_hex();

        // Clear
        storage.clear().unwrap();

        assert_eq!(storage.height(), 0);
        assert_eq!(storage.utxo_count(), 0);
        assert_ne!(storage.muhash_hex(), muhash_before_clear);

        // Should be able to apply block again
        storage.apply_block(&block, 0).unwrap();
        assert_eq!(storage.utxo_count(), 1);
    }

    #[test]
    fn test_iter_utxos() {
        let storage = BitcoinState::open_temp().unwrap();

        let utxos = create_test_utxos(5);
        storage.bulk_import(utxos.clone()).unwrap();

        // Iterate and count
        let iterated: Vec<_> = storage.iter_utxos().collect();
        assert_eq!(iterated.len(), 5);

        // All UTXOs should be present
        for (outpoint, coin) in &utxos {
            let found = iterated.iter().any(|(op, c)| op == outpoint && c == coin);
            assert!(found, "UTXO {outpoint:?} not found in iteration");
        }
    }

    #[test]
    fn test_sync_roundtrip() {
        // Simulate a full sync: export from one storage, import to another
        let source = BitcoinState::open_temp().unwrap();
        let target = BitcoinState::open_temp().unwrap();

        // Source has blocks
        let block0 = create_test_block(0, &[]);
        source.apply_block(&block0, 0).unwrap();

        let coinbase0 = OutPoint {
            txid: block0.txdata[0].compute_txid(),
            vout: 0,
        };
        let block1 = create_test_block(1, &[coinbase0]);
        source.apply_block(&block1, 1).unwrap();

        let source_height = source.height();
        let source_muhash = source.muhash_hex();
        let source_count = source.utxo_count();

        // Export all UTXOs from source
        let (utxos, _, is_complete) = source.export_chunk(None, 1000).unwrap();
        assert!(is_complete);

        // Import to target
        target.bulk_import(utxos).unwrap();
        target.finalize_import(source_height).unwrap();

        // Verify target matches source
        assert_eq!(target.height(), source_height);
        assert_eq!(target.utxo_count(), source_count);
        assert_eq!(target.muhash_hex(), source_muhash);
    }

    #[test]
    fn test_verify_muhash() {
        let storage = BitcoinState::open_temp().unwrap();

        let block = create_test_block(0, &[]);
        storage.apply_block(&block, 0).unwrap();

        let muhash = storage.muhash_hex();

        // Correct verification
        assert!(storage.verify_muhash(&muhash));

        // Incorrect verification
        assert!(!storage
            .verify_muhash("0000000000000000000000000000000000000000000000000000000000000000"));
    }

    #[test]
    fn test_outpoint_key_roundtrip() {
        let outpoint = OutPoint {
            txid: bitcoin::Txid::all_zeros(),
            vout: 42,
        };

        let key = outpoint_to_key(&outpoint);
        let decoded = key_to_outpoint(&key);

        assert_eq!(outpoint, decoded);
    }
}
