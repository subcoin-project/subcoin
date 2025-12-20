//! Native UTXO storage implementation using RocksDB with MuHash commitment.

use crate::coin::{outpoint_to_key, Coin};
use crate::undo::BlockUndo;
use crate::{cf, meta_keys, Error, Result};
use bitcoin::{Block, OutPoint, Transaction};
use parking_lot::RwLock;
use rocksdb::{ColumnFamily, ColumnFamilyDescriptor, Options, WriteBatch, DB};
use std::path::Path;
use subcoin_crypto::MuHash3072;

/// Native UTXO storage with MuHash commitment tracking.
///
/// This provides O(1) UTXO operations and O(1) MuHash updates per UTXO change,
/// compared to O(log n) for Substrate's Merkle Patricia Trie.
pub struct NativeUtxoStorage {
    /// RocksDB instance.
    db: DB,
    /// Rolling MuHash accumulator (protected by RwLock for concurrent reads).
    muhash: RwLock<MuHash3072>,
    /// Total UTXO count.
    utxo_count: RwLock<u64>,
    /// Current block height.
    height: RwLock<u32>,
}

impl NativeUtxoStorage {
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
            "Opened native UTXO storage at height {height}, {utxo_count} UTXOs, muhash: {}",
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
            .and_then(|bytes| Coin::decode(&bytes).ok())
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
    ) -> Result<()> {
        let txid = tx.compute_txid();
        let is_coinbase = tx.is_coinbase();

        // Process inputs (spend UTXOs) - skip for coinbase
        if !is_coinbase {
            for input in &tx.input {
                let outpoint = input.previous_output;
                let key = outpoint_to_key(&outpoint);

                // Get existing UTXO
                let coin_bytes = self
                    .db
                    .get_cf(cf_utxos, key)
                    .map_err(Error::Rocksdb)?
                    .ok_or(Error::UtxoNotFound(outpoint))?;

                let coin =
                    Coin::decode(&coin_bytes).map_err(|e| Error::Deserialization(e.to_string()))?;

                // Record for undo
                undo.record_spend(outpoint, coin.clone());

                // Remove from MuHash
                let muhash_data = coin.serialize_for_muhash(&outpoint);
                muhash.remove(&muhash_data);

                // Remove from storage
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

            // Record for undo
            undo.record_create(outpoint);

            // Add to MuHash
            let muhash_data = coin.serialize_for_muhash(&outpoint);
            muhash.insert(&muhash_data);

            // Add to storage
            batch.put_cf(cf_utxos, key, coin.encode());
            *created += 1;
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
            .get_cf(cf_undo, undo_key)
            .map_err(Error::Rocksdb)?
            .ok_or(Error::UndoNotFound(height))?;

        let undo =
            BlockUndo::decode(&undo_bytes).map_err(|e| Error::Deserialization(e.to_string()))?;

        let mut batch = WriteBatch::default();
        let mut muhash = self.muhash.write();

        // Remove created UTXOs
        for outpoint in &undo.created_outpoints {
            let key = outpoint_to_key(outpoint);

            // Get the coin to remove from MuHash
            if let Some(coin_bytes) = self.db.get_cf(&cf_utxos, key).map_err(Error::Rocksdb)? {
                let coin =
                    Coin::decode(&coin_bytes).map_err(|e| Error::Deserialization(e.to_string()))?;
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
            batch.put_cf(&cf_utxos, key, coin.encode());
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

    /// Get current UTXO count.
    pub fn utxo_count(&self) -> u64 {
        *self.utxo_count.read()
    }

    /// Get current block height.
    pub fn height(&self) -> u32 {
        *self.height.read()
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
        let storage = NativeUtxoStorage::open_temp().unwrap();

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
        let storage = NativeUtxoStorage::open_temp().unwrap();

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
        let storage = NativeUtxoStorage::open_temp().unwrap();

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
}
