//! Block preparation and verification abstractions.
//!
//! This module provides data structures for efficient block verification:
//! - [`UtxoProvider`]: Trait for abstracting UTXO access from different backends
//! - [`TxExecutionLevels`]: Groups transactions by parallel execution level
//! - [`PreparedBlock`]: Block with pre-fetched UTXOs ready for verification
//! - [`VerifiedBlock`]: A block that passed verification, ready to apply to state
//!
//! # Design Goals
//!
//! 1. **Eliminate duplicate UTXO lookups**: Fetch UTXOs once during prepare, reuse in verify and apply
//! 2. **Enable batch fetching**: Single batch read instead of N individual reads
//! 3. **Support parallel verification**: TxExecutionLevels identifies independent transactions
//! 4. **Backend agnostic**: UtxoProvider works with Substrate state or native storage

use bitcoin::{Block, OutPoint, TxOut};
use std::collections::{HashMap, HashSet};
use subcoin_primitives::runtime::Coin;

/// Abstraction for UTXO access.
///
/// This trait decouples block verification from storage implementation,
/// enabling:
/// - Substrate state lookups during normal operation
/// - Native RocksDB lookups for optimized storage
/// - Pre-fetched data during batch verification
/// - Cached lookups for improved performance
pub trait UtxoProvider {
    /// Get a single UTXO by outpoint.
    ///
    /// Returns `None` if the UTXO doesn't exist.
    fn get(&self, outpoint: &OutPoint) -> Option<Coin>;

    /// Batch-fetch multiple UTXOs.
    ///
    /// Returns a map containing only the UTXOs that were found.
    /// Missing outpoints are silently omitted from the result.
    ///
    /// Default implementation calls `get()` for each outpoint.
    /// Implementations should override for better performance.
    fn batch_get(&self, outpoints: &[OutPoint]) -> HashMap<OutPoint, Coin> {
        outpoints
            .iter()
            .filter_map(|op| self.get(op).map(|coin| (*op, coin)))
            .collect()
    }
}

/// Implementation for HashMap (used with pre-fetched UTXOs).
///
/// This allows verification to use pre-fetched data without
/// any storage access.
impl UtxoProvider for HashMap<OutPoint, Coin> {
    fn get(&self, outpoint: &OutPoint) -> Option<Coin> {
        HashMap::get(self, outpoint).cloned()
    }

    fn batch_get(&self, outpoints: &[OutPoint]) -> HashMap<OutPoint, Coin> {
        outpoints
            .iter()
            .filter_map(|op| HashMap::get(self, op).map(|coin| (*op, coin.clone())))
            .collect()
    }
}

/// Transactions grouped by parallel execution level.
///
/// - Level 0: No in-block dependencies (coinbase, txs spending only external UTXOs)
/// - Level 1: Depends only on level 0 transactions
/// - Level N: Depends only on levels < N
///
/// All transactions within the same level can execute in parallel.
#[derive(Debug, Clone)]
pub struct TxExecutionLevels {
    /// levels[i] contains indices of transactions that can run at level i.
    levels: Vec<Vec<usize>>,
}

impl TxExecutionLevels {
    /// Build execution levels by analyzing in-block dependencies.
    ///
    /// A transaction at index `i` depends on transaction at index `j` (where `j < i`)
    /// if transaction `i` spends an output created by transaction `j`.
    pub fn build(block: &Block) -> Self {
        let tx_count = block.txdata.len();

        if tx_count == 0 {
            return Self { levels: vec![] };
        }

        // Track which outputs are created by which transaction
        let mut output_creators: HashMap<OutPoint, usize> = HashMap::new();

        // Track dependencies for each transaction
        // dependencies[i] = set of tx indices that tx i depends on
        let mut dependencies: Vec<HashSet<usize>> = vec![HashSet::new(); tx_count];

        for (tx_idx, tx) in block.txdata.iter().enumerate() {
            let txid = tx.compute_txid();

            // Record outputs created by this transaction
            for vout in 0..tx.output.len() {
                let outpoint = OutPoint {
                    txid,
                    vout: vout as u32,
                };
                output_creators.insert(outpoint, tx_idx);
            }

            // Check if any input is from a tx in this block (skip coinbase)
            if tx_idx > 0 {
                for input in &tx.input {
                    if let Some(&creator_idx) = output_creators.get(&input.previous_output) {
                        dependencies[tx_idx].insert(creator_idx);
                    }
                }
            }
        }

        // Build levels using topological sort
        Self::topological_levels(&dependencies)
    }

    /// Convert dependencies into parallel execution levels.
    fn topological_levels(dependencies: &[HashSet<usize>]) -> Self {
        let n = dependencies.len();

        if n == 0 {
            return Self { levels: vec![] };
        }

        // Compute in-degree (number of dependencies) for each tx
        let mut in_degree: Vec<usize> = dependencies.iter().map(|deps| deps.len()).collect();

        // Track which transactions depend on each transaction
        let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (tx_idx, deps) in dependencies.iter().enumerate() {
            for &dep in deps {
                dependents[dep].push(tx_idx);
            }
        }

        let mut levels = Vec::new();
        let mut remaining: HashSet<usize> = (0..n).collect();

        while !remaining.is_empty() {
            // Find all txs with no remaining dependencies (in_degree == 0)
            let level: Vec<usize> = remaining
                .iter()
                .filter(|&&idx| in_degree[idx] == 0)
                .copied()
                .collect();

            if level.is_empty() {
                // This shouldn't happen in valid Bitcoin blocks
                // (would indicate a cycle, which is impossible)
                panic!("Cycle detected in transaction dependencies - invalid block");
            }

            // Remove processed txs and update in-degrees
            for &tx_idx in &level {
                remaining.remove(&tx_idx);
                for &dependent in &dependents[tx_idx] {
                    in_degree[dependent] = in_degree[dependent].saturating_sub(1);
                }
            }

            levels.push(level);
        }

        Self { levels }
    }

    /// Returns the number of execution levels.
    ///
    /// A value of 1 means all transactions can run in parallel.
    /// Higher values indicate more sequential dependencies.
    pub fn depth(&self) -> usize {
        self.levels.len()
    }

    /// Returns true if all transactions can execute in parallel (single level).
    pub fn is_fully_parallel(&self) -> bool {
        self.levels.len() <= 1
    }

    /// Iterate over levels for parallel execution.
    pub fn iter(&self) -> impl Iterator<Item = &[usize]> {
        self.levels.iter().map(|v| v.as_slice())
    }

    /// Get a specific level.
    pub fn get_level(&self, level: usize) -> Option<&[usize]> {
        self.levels.get(level).map(|v| v.as_slice())
    }

    /// Total number of transactions across all levels.
    pub fn tx_count(&self) -> usize {
        self.levels.iter().map(|l| l.len()).sum()
    }
}

/// In-block output information.
#[derive(Debug, Clone)]
pub struct InBlockOutput {
    /// The transaction output.
    pub txout: TxOut,
    /// Whether this is from the coinbase transaction.
    pub is_coinbase: bool,
    /// Index of the transaction that created this output.
    pub tx_index: usize,
}

/// A block with pre-fetched UTXOs ready for verification.
///
/// This structure enables efficient verification by:
/// 1. Pre-fetching all required UTXOs in a single batch
/// 2. Tracking in-block outputs for same-block spending
/// 3. Providing execution levels for parallel verification
#[derive(Debug)]
pub struct PreparedBlock {
    /// The Bitcoin block.
    pub block: Block,
    /// Block height.
    pub height: u32,
    /// Pre-fetched UTXOs for inputs spending external outputs.
    /// Key: OutPoint being spent
    /// Value: The Coin (UTXO) data
    pub input_utxos: HashMap<OutPoint, Coin>,
    /// Outputs created within this block (for in-block spending).
    pub in_block_outputs: HashMap<OutPoint, InBlockOutput>,
    /// Transaction execution levels for intra-block parallelism.
    pub execution_levels: TxExecutionLevels,
}

impl PreparedBlock {
    /// Prepare a block for verification by pre-fetching required UTXOs.
    ///
    /// This method:
    /// 1. Scans the block to identify all required outpoints
    /// 2. Separates external UTXOs from in-block outputs
    /// 3. Batch-fetches external UTXOs from the provider
    /// 4. Builds execution levels for parallel verification
    pub fn prepare<P: UtxoProvider>(block: Block, height: u32, utxo_provider: &P) -> Self {
        // First pass: collect in-block outputs
        let mut in_block_outputs: HashMap<OutPoint, InBlockOutput> = HashMap::new();

        for (tx_idx, tx) in block.txdata.iter().enumerate() {
            let txid = tx.compute_txid();
            let is_coinbase = tx_idx == 0;

            for (vout, txout) in tx.output.iter().enumerate() {
                let outpoint = OutPoint {
                    txid,
                    vout: vout as u32,
                };
                in_block_outputs.insert(
                    outpoint,
                    InBlockOutput {
                        txout: txout.clone(),
                        is_coinbase,
                        tx_index: tx_idx,
                    },
                );
            }
        }

        // Second pass: collect external outpoints to fetch
        let mut external_outpoints: Vec<OutPoint> = Vec::new();

        for tx in block.txdata.iter().skip(1) {
            // Skip coinbase
            for input in &tx.input {
                if !in_block_outputs.contains_key(&input.previous_output) {
                    external_outpoints.push(input.previous_output);
                }
            }
        }

        // Batch-fetch external UTXOs
        let input_utxos = utxo_provider.batch_get(&external_outpoints);

        // Build execution levels
        let execution_levels = TxExecutionLevels::build(&block);

        Self {
            block,
            height,
            input_utxos,
            in_block_outputs,
            execution_levels,
        }
    }

    /// Get a UTXO needed for verification.
    ///
    /// Checks in-block outputs first, then pre-fetched external UTXOs.
    /// Returns (TxOut, is_coinbase, coin_height).
    pub fn get_utxo(&self, outpoint: &OutPoint) -> Option<(TxOut, bool, u32)> {
        // Check in-block outputs first
        if let Some(in_block) = self.in_block_outputs.get(outpoint) {
            return Some((in_block.txout.clone(), in_block.is_coinbase, self.height));
        }

        // Check pre-fetched external UTXOs
        if let Some(coin) = self.input_utxos.get(outpoint) {
            let txout = TxOut {
                value: bitcoin::Amount::from_sat(coin.amount),
                script_pubkey: bitcoin::ScriptBuf::from_bytes(coin.script_pubkey.clone()),
            };
            return Some((txout, coin.is_coinbase, coin.height));
        }

        None
    }

    /// Check if all required external UTXOs were found.
    ///
    /// Returns a list of missing outpoints if any are missing.
    pub fn missing_utxos(&self) -> Vec<OutPoint> {
        let mut missing = Vec::new();

        for tx in self.block.txdata.iter().skip(1) {
            for input in &tx.input {
                let outpoint = &input.previous_output;
                if !self.in_block_outputs.contains_key(outpoint)
                    && !self.input_utxos.contains_key(outpoint)
                {
                    missing.push(*outpoint);
                }
            }
        }

        missing
    }

    /// Returns the number of transactions in the block.
    pub fn tx_count(&self) -> usize {
        self.block.txdata.len()
    }

    /// Returns the block hash.
    pub fn block_hash(&self) -> bitcoin::BlockHash {
        self.block.block_hash()
    }

    /// Convert to a VerifiedBlock after successful verification.
    ///
    /// This transfers ownership of the pre-fetched UTXOs to the VerifiedBlock,
    /// allowing apply_verified_block to use them without re-fetching.
    pub fn into_verified(self, script_verification_duration: std::time::Duration) -> VerifiedBlock {
        VerifiedBlock {
            block: self.block,
            height: self.height,
            spent_utxos: self.input_utxos,
            script_verification_duration,
        }
    }
}

/// A block that has passed verification, ready to apply to state.
///
/// This structure carries the pre-fetched UTXOs from verification,
/// eliminating duplicate lookups when applying the block.
#[derive(Debug)]
pub struct VerifiedBlock {
    /// The Bitcoin block.
    pub block: Block,
    /// Block height.
    pub height: u32,
    /// Pre-fetched UTXOs that were spent by this block.
    /// These are the external UTXOs (not in-block spending).
    /// Passed through from verification so apply doesn't need to re-fetch.
    pub spent_utxos: HashMap<OutPoint, Coin>,
    /// Time spent on script verification.
    pub script_verification_duration: std::time::Duration,
}

impl VerifiedBlock {
    /// Returns the block hash.
    pub fn block_hash(&self) -> bitcoin::BlockHash {
        self.block.block_hash()
    }

    /// Get a spent UTXO by outpoint.
    ///
    /// Only contains external UTXOs (not in-block outputs).
    pub fn get_spent_utxo(&self, outpoint: &OutPoint) -> Option<&Coin> {
        self.spent_utxos.get(outpoint)
    }

    /// Returns the number of transactions in the block.
    pub fn tx_count(&self) -> usize {
        self.block.txdata.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::absolute::LockTime;
    use bitcoin::blockdata::block::{Header, Version};
    use bitcoin::blockdata::transaction::{Transaction, TxIn, Version as TxVersion};
    use bitcoin::hashes::Hash;
    use bitcoin::{Amount, CompactTarget, ScriptBuf, Sequence, Witness};

    fn create_coinbase(height: u32) -> Transaction {
        let script = vec![0x01, height as u8];
        Transaction {
            version: TxVersion::TWO,
            lock_time: LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::null(),
                script_sig: ScriptBuf::from_bytes(script),
                sequence: Sequence::MAX,
                witness: Witness::new(),
            }],
            output: vec![bitcoin::TxOut {
                value: Amount::from_sat(5_000_000_000),
                script_pubkey: ScriptBuf::new_p2pkh(&bitcoin::PubkeyHash::all_zeros()),
            }],
        }
    }

    fn create_spending_tx(inputs: &[OutPoint]) -> Transaction {
        Transaction {
            version: TxVersion::TWO,
            lock_time: LockTime::ZERO,
            input: inputs
                .iter()
                .map(|op| TxIn {
                    previous_output: *op,
                    script_sig: ScriptBuf::new(),
                    sequence: Sequence::MAX,
                    witness: Witness::new(),
                })
                .collect(),
            output: vec![bitcoin::TxOut {
                value: Amount::from_sat(1_000_000),
                script_pubkey: ScriptBuf::new_p2pkh(&bitcoin::PubkeyHash::all_zeros()),
            }],
        }
    }

    fn create_block(txs: Vec<Transaction>) -> Block {
        Block {
            header: Header {
                version: Version::TWO,
                prev_blockhash: bitcoin::BlockHash::all_zeros(),
                merkle_root: bitcoin::TxMerkleNode::all_zeros(),
                time: 0,
                bits: CompactTarget::from_consensus(0),
                nonce: 0,
            },
            txdata: txs,
        }
    }

    fn test_coin(amount: u64) -> Coin {
        Coin {
            is_coinbase: false,
            amount,
            height: 100,
            script_pubkey: vec![0x51],
        }
    }

    #[test]
    fn test_hashmap_provider() {
        let mut utxos = HashMap::new();
        let outpoint1 = OutPoint {
            txid: bitcoin::Txid::from_byte_array([1; 32]),
            vout: 0,
        };
        let outpoint2 = OutPoint {
            txid: bitcoin::Txid::from_byte_array([2; 32]),
            vout: 0,
        };
        utxos.insert(outpoint1, test_coin(1000));
        utxos.insert(outpoint2, test_coin(2000));

        // Test get
        assert!(utxos.get(&outpoint1).is_some());
        let missing = OutPoint {
            txid: bitcoin::Txid::from_byte_array([99; 32]),
            vout: 0,
        };
        assert!(UtxoProvider::get(&utxos, &missing).is_none());

        // Test batch_get
        let result = utxos.batch_get(&[outpoint1, outpoint2, missing]);
        assert_eq!(result.len(), 2);
        assert!(result.contains_key(&outpoint1));
        assert!(result.contains_key(&outpoint2));
        assert!(!result.contains_key(&missing));
    }

    #[test]
    fn test_prepared_block_external_utxos() {
        let coinbase = create_coinbase(1);

        let external_utxo = OutPoint {
            txid: bitcoin::Txid::from_byte_array([99; 32]),
            vout: 0,
        };

        let tx1 = create_spending_tx(&[external_utxo]);
        let block = create_block(vec![coinbase, tx1]);

        // Create mock UTXO provider with the external UTXO
        let mut utxos: HashMap<OutPoint, Coin> = HashMap::new();
        utxos.insert(
            external_utxo,
            Coin {
                is_coinbase: false,
                amount: 5_000_000,
                height: 50,
                script_pubkey: vec![0x51],
            },
        );

        let prepared = PreparedBlock::prepare(block, 100, &utxos);

        // Should have the external UTXO pre-fetched
        assert!(prepared.input_utxos.contains_key(&external_utxo));
        assert!(prepared.missing_utxos().is_empty());

        // get_utxo should return the external UTXO with correct height
        let (_, is_coinbase, height) = prepared.get_utxo(&external_utxo).unwrap();
        assert!(!is_coinbase);
        assert_eq!(height, 50); // Original height, not block height
    }

    #[test]
    fn test_prepared_block_in_block_spending() {
        let coinbase = create_coinbase(1);
        let coinbase_out = OutPoint {
            txid: coinbase.compute_txid(),
            vout: 0,
        };

        // tx1 spends coinbase (in-block)
        let tx1 = create_spending_tx(&[coinbase_out]);
        let block = create_block(vec![coinbase, tx1]);

        // Empty provider - no external UTXOs needed
        let utxos: HashMap<OutPoint, Coin> = HashMap::new();

        let prepared = PreparedBlock::prepare(block, 100, &utxos);

        // Coinbase output should be in in_block_outputs
        assert!(prepared.in_block_outputs.contains_key(&coinbase_out));
        assert!(prepared.missing_utxos().is_empty());

        // get_utxo should return it with block height
        let (_, is_coinbase, height) = prepared.get_utxo(&coinbase_out).unwrap();
        assert!(is_coinbase);
        assert_eq!(height, 100); // Block height for in-block outputs
    }

    #[test]
    fn test_prepared_block_missing_utxo() {
        let coinbase = create_coinbase(1);

        let external_utxo = OutPoint {
            txid: bitcoin::Txid::from_byte_array([99; 32]),
            vout: 0,
        };

        let tx1 = create_spending_tx(&[external_utxo]);
        let block = create_block(vec![coinbase, tx1]);

        // Empty provider - UTXO not found
        let utxos: HashMap<OutPoint, Coin> = HashMap::new();

        let prepared = PreparedBlock::prepare(block, 100, &utxos);

        // Should report missing UTXO
        let missing = prepared.missing_utxos();
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0], external_utxo);
    }

    // TxExecutionLevels tests

    #[test]
    fn test_execution_levels_no_dependencies() {
        // Block with coinbase + 3 independent txs (all spend external UTXOs)
        let coinbase = create_coinbase(1);

        let external_utxo1 = OutPoint {
            txid: bitcoin::Txid::from_byte_array([1; 32]),
            vout: 0,
        };
        let external_utxo2 = OutPoint {
            txid: bitcoin::Txid::from_byte_array([2; 32]),
            vout: 0,
        };
        let external_utxo3 = OutPoint {
            txid: bitcoin::Txid::from_byte_array([3; 32]),
            vout: 0,
        };

        let tx1 = create_spending_tx(&[external_utxo1]);
        let tx2 = create_spending_tx(&[external_utxo2]);
        let tx3 = create_spending_tx(&[external_utxo3]);

        let block = create_block(vec![coinbase, tx1, tx2, tx3]);
        let levels = TxExecutionLevels::build(&block);

        // All 4 txs should be in level 0 (no dependencies)
        assert_eq!(levels.depth(), 1);
        assert!(levels.is_fully_parallel());
        assert_eq!(levels.tx_count(), 4);

        let level0 = levels.get_level(0).unwrap();
        assert_eq!(level0.len(), 4);
    }

    #[test]
    fn test_execution_levels_chain_dependency() {
        // tx0 (coinbase) -> tx1 -> tx2 -> tx3 (chain of dependencies)
        let coinbase = create_coinbase(1);
        let coinbase_out = OutPoint {
            txid: coinbase.compute_txid(),
            vout: 0,
        };

        let tx1 = create_spending_tx(&[coinbase_out]);
        let tx1_out = OutPoint {
            txid: tx1.compute_txid(),
            vout: 0,
        };

        let tx2 = create_spending_tx(&[tx1_out]);
        let tx2_out = OutPoint {
            txid: tx2.compute_txid(),
            vout: 0,
        };

        let tx3 = create_spending_tx(&[tx2_out]);

        let block = create_block(vec![coinbase, tx1, tx2, tx3]);
        let levels = TxExecutionLevels::build(&block);

        // Should have 4 levels: [coinbase], [tx1], [tx2], [tx3]
        assert_eq!(levels.depth(), 4);
        assert!(!levels.is_fully_parallel());

        assert_eq!(levels.get_level(0).unwrap(), &[0]); // coinbase
        assert_eq!(levels.get_level(1).unwrap(), &[1]); // tx1
        assert_eq!(levels.get_level(2).unwrap(), &[2]); // tx2
        assert_eq!(levels.get_level(3).unwrap(), &[3]); // tx3
    }

    #[test]
    fn test_execution_levels_diamond_dependency() {
        // Diamond pattern:
        //       coinbase
        //       /      \
        //     tx1      tx2  (both spend coinbase, can run in parallel)
        //       \      /
        //         tx3      (spends both tx1 and tx2)

        let coinbase = create_coinbase(1);

        // Coinbase with 2 outputs
        let mut coinbase_2out = coinbase.clone();
        coinbase_2out.output.push(bitcoin::TxOut {
            value: Amount::from_sat(1_000_000),
            script_pubkey: ScriptBuf::new_p2pkh(&bitcoin::PubkeyHash::all_zeros()),
        });

        let coinbase_out0 = OutPoint {
            txid: coinbase_2out.compute_txid(),
            vout: 0,
        };
        let coinbase_out1 = OutPoint {
            txid: coinbase_2out.compute_txid(),
            vout: 1,
        };

        let tx1 = create_spending_tx(&[coinbase_out0]);
        let tx1_out = OutPoint {
            txid: tx1.compute_txid(),
            vout: 0,
        };

        let tx2 = create_spending_tx(&[coinbase_out1]);
        let tx2_out = OutPoint {
            txid: tx2.compute_txid(),
            vout: 0,
        };

        let tx3 = create_spending_tx(&[tx1_out, tx2_out]);

        let block = create_block(vec![coinbase_2out, tx1, tx2, tx3]);
        let levels = TxExecutionLevels::build(&block);

        // Should have 3 levels:
        // Level 0: [coinbase]
        // Level 1: [tx1, tx2] - both depend only on coinbase
        // Level 2: [tx3] - depends on tx1 and tx2
        assert_eq!(levels.depth(), 3);

        assert_eq!(levels.get_level(0).unwrap(), &[0]);
        let level1 = levels.get_level(1).unwrap();
        assert_eq!(level1.len(), 2);
        assert!(level1.contains(&1));
        assert!(level1.contains(&2));
        assert_eq!(levels.get_level(2).unwrap(), &[3]);
    }

    #[test]
    fn test_empty_block() {
        let block = create_block(vec![]);
        let levels = TxExecutionLevels::build(&block);

        assert_eq!(levels.depth(), 0);
        assert!(levels.is_fully_parallel());
        assert_eq!(levels.tx_count(), 0);
    }

    #[test]
    fn test_coinbase_only_block() {
        let coinbase = create_coinbase(1);
        let block = create_block(vec![coinbase]);

        let levels = TxExecutionLevels::build(&block);

        assert_eq!(levels.depth(), 1);
        assert!(levels.is_fully_parallel());
        assert_eq!(levels.tx_count(), 1);
    }
}
