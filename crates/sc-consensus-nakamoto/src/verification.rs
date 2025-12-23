//! This module provides block verification functionalities based on Bitcoin's consensus rules.
//! The primary code reference for these consensus rules is Bitcoin Core.
//!
//! We utilize the `rust-bitcoinconsensus` from `rust-bitcoin` for handling the most complex
//! aspects of script verification.
//!
//! The main components of this module are:
//! - `header_verify`: Module responsible for verifying block headers.
//! - `tx_verify`: Module responsible for verifying individual transactions within a block.
//!
//! This module ensures that blocks adhere to Bitcoin's consensus rules by performing checks on
//! the proof of work, timestamps, transaction validity, and more.
//!
//! # Components
//!
//! ## Modules
//!
//! - `header_verify`: Contains functions and structures for verifying block headers.
//! - `tx_verify`: Contains functions and structures for verifying transactions.
//!
//! ## Structures
//!
//! - [`BlockVerifier`]: Responsible for verifying Bitcoin blocks, including headers and transactions.
//!
//! ## Enums
//!
//! - [`BlockVerification`]: Represents the level of block verification (None, Full, HeaderOnly).

mod header_verify;
mod script_verify;
mod tx_verify;

use crate::chain_params::ChainParams;
use bitcoin::block::Bip34Error;
use bitcoin::blockdata::block::Header as BitcoinHeader;
use bitcoin::blockdata::constants::{COINBASE_MATURITY, MAX_BLOCK_SIGOPS_COST};
use bitcoin::blockdata::weight::WITNESS_SCALE_FACTOR;
use bitcoin::consensus::Encodable;
use bitcoin::hashes::Hash;
use bitcoin::{
    Amount, Block as BitcoinBlock, BlockHash, OutPoint, ScriptBuf, TxMerkleNode, TxOut, Txid,
    VarInt, Weight,
};
pub use header_verify::{Error as HeaderError, HeaderVerifier};
use sc_client_api::{AuxStore, Backend, StorageProvider};
use sp_blockchain::HeaderBackend;
use sp_runtime::traits::Block as BlockT;
use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use std::sync::Arc;
use subcoin_bitcoin_state::BitcoinState;
use subcoin_primitives::MAX_BLOCK_WEIGHT;
use subcoin_primitives::consensus::{TxError, check_transaction_sanity};
use subcoin_primitives::runtime::{Coin, bitcoin_block_subsidy};
use tx_verify::{get_legacy_sig_op_count, is_final_tx};

/// Represents the Bitcoin script backend.
#[derive(Copy, Default, Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "cli", derive(clap::ValueEnum))]
pub enum ScriptEngine {
    /// Uses the Bitcoin Core bindings for script verification.
    #[default]
    Core,
    /// No script verification.
    None,
    /// Uses the Rust-based Bitcoin script interpreter (Subcoin) for script verification.
    /// This is an experimental feature, not yet fully validated for production use.
    Subcoin,
}

/// Represents the level of block verification.
#[derive(Copy, Default, Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "cli", derive(clap::ValueEnum))]
pub enum BlockVerification {
    /// No verification performed.
    None,
    /// Full verification, including verifying the transactions.
    #[default]
    Full,
    /// Verify the block header only, without the transaction veification.
    HeaderOnly,
}

/// Represents the context of a transaction within a block.
#[derive(Debug, Clone)]
pub struct TransactionContext {
    /// Block number containing the transaction.
    pub block_number: u32,
    /// Index of the transaction in the block.
    pub tx_index: usize,
    /// ID of the transaction.
    pub txid: Txid,
}

impl std::fmt::Display for TransactionContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Tx #{}:{}: {}",
            self.block_number, self.tx_index, self.txid
        )
    }
}

/// A script verification task to be executed in parallel.
///
/// This struct captures all data needed to verify a single input's script
/// independently of other inputs.
struct ScriptVerificationTask {
    /// Serialized spending transaction (for Core engine).
    spending_tx_bytes: Vec<u8>,
    /// Transaction context for error reporting.
    tx_context: TransactionContext,
    /// Input index within the transaction.
    input_index: usize,
    /// The spent output (contains script_pubkey to verify against).
    spent_output: TxOut,
    /// Script verification flags.
    flags: u32,
    /// Full transaction (for Subcoin engine only).
    tx: Option<bitcoin::Transaction>,
}

/// Verifies a single script verification task.
///
/// This function is designed to be called from multiple threads.
fn verify_script_task(
    task: &ScriptVerificationTask,
    script_engine: ScriptEngine,
) -> Result<(), Error> {
    match script_engine {
        ScriptEngine::Core => script_verify::verify_input_script(
            &task.spent_output,
            &task.spending_tx_bytes,
            task.input_index,
            task.flags,
        ),
        ScriptEngine::Subcoin => {
            let tx = task
                .tx
                .as_ref()
                .expect("Transaction must be present for Subcoin engine; qed");
            let input = &tx.input[task.input_index];

            let mut checker = subcoin_script::TransactionSignatureChecker::new(
                tx,
                task.input_index,
                task.spent_output.value.to_sat(),
            );

            let verify_flags =
                subcoin_script::VerifyFlags::from_bits(task.flags).expect("Invalid flags");

            subcoin_script::verify_script(
                &input.script_sig,
                &task.spent_output.script_pubkey,
                &input.witness,
                &verify_flags,
                &mut checker,
            )
            .map_err(|script_err| Error::InvalidScript {
                block_hash: BlockHash::all_zeros(),
                context: task.tx_context.clone(),
                input_index: task.input_index,
                error: Box::new(script_err),
            })
        }
        ScriptEngine::None => Ok(()),
    }
}

/// Verifies all script tasks in parallel using rayon.
fn verify_scripts_parallel(
    tasks: &[ScriptVerificationTask],
    script_engine: ScriptEngine,
    block_hash: BlockHash,
) -> Result<(), Error> {
    use rayon::prelude::*;

    if matches!(script_engine, ScriptEngine::None) {
        return Ok(());
    }

    tasks.par_iter().try_for_each(|task| {
        verify_script_task(task, script_engine).map_err(|err| {
            // Update block_hash in error if needed
            match err {
                Error::InvalidScript {
                    context,
                    input_index,
                    error,
                    ..
                } => Error::InvalidScript {
                    block_hash,
                    context,
                    input_index,
                    error,
                },
                other => other,
            }
        })
    })
}

/// Verifies all script tasks sequentially.
fn verify_scripts_sequential(
    tasks: &[ScriptVerificationTask],
    script_engine: ScriptEngine,
    block_hash: BlockHash,
) -> Result<(), Error> {
    if matches!(script_engine, ScriptEngine::None) {
        return Ok(());
    }

    for task in tasks {
        verify_script_task(task, script_engine).map_err(|err| match err {
            Error::InvalidScript {
                context,
                input_index,
                error,
                ..
            } => Error::InvalidScript {
                block_hash,
                context,
                input_index,
                error,
            },
            other => other,
        })?;
    }

    Ok(())
}

/// Block verification error.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The merkle root of the block is invalid.
    #[error("Invalid merkle root in block#{0}")]
    BadMerkleRoot(BlockHash),
    /// Block must contain at least one coinbase transaction.
    #[error("Block#{0} has an empty transaction list")]
    EmptyTransactionList(BlockHash),
    /// Block size exceeds the limit.
    #[error("Block#{0} exceeds maximum allowed size")]
    BlockTooLarge(BlockHash),
    /// First transaction must be a coinbase.
    #[error("First transaction in block#{0} is not a coinbase")]
    FirstTransactionIsNotCoinbase(BlockHash),
    /// A block must contain only one coinbase transaction.
    #[error("Block#{0} contains multiple coinbase transactions")]
    MultipleCoinbase(BlockHash),
    #[error("Block#{block_hash} has incorrect coinbase height, (expected: {expected}, got: {got})")]
    BadCoinbaseBlockHeight {
        block_hash: BlockHash,
        got: u32,
        expected: u32,
    },
    /// Coinbase transaction is prematurely spent.
    #[error("Premature spend of coinbase transaction in block#{0}")]
    PrematureSpendOfCoinbase(BlockHash),

    /// Block contains duplicate transactions.
    #[error("Block#{block_hash} contains duplicate transaction at index {index}")]
    DuplicateTransaction { block_hash: BlockHash, index: usize },
    /// A transaction is not finalized.
    #[error("Transaction in block#{0} is not finalized")]
    TransactionNotFinal(BlockHash),
    /// Transaction script contains too many signature operations.
    #[error(
        "Transaction in block #{block_number},{block_hash} exceeds signature operation limit (max: {MAX_BLOCK_SIGOPS_COST})"
    )]
    TooManySigOps {
        block_hash: BlockHash,
        block_number: u32,
    },
    /// Witness commitment in the block is invalid.
    #[error("Invalid witness commitment in block#{0}")]
    BadWitnessCommitment(BlockHash),

    /// UTXO referenced by a transaction is missing
    #[error(
        "Missing UTXO in state for transaction ({context}) in block {block_hash}. Missing UTXO: {missing_utxo:?}"
    )]
    MissingUtxoInState {
        block_hash: BlockHash,
        /// Context of the transaction being processed.
        context: TransactionContext,
        /// UTXO missing from the UTXO set.
        missing_utxo: OutPoint,
    },
    /// UTXO has already been spent within the same block.
    #[error("UTXO already spent in current block (#{block_number},{block_hash}:{txid}: {utxo:?})")]
    AlreadySpentInCurrentBlock {
        block_hash: BlockHash,
        block_number: u32,
        txid: Txid,
        utxo: OutPoint,
    },
    /// Insufficient funds: total input amount is lower than the total output amount.
    #[error(
        "Block#{block_hash} has an invalid transaction: total input {value_in} < total output {value_out}"
    )]
    InsufficientFunds {
        block_hash: BlockHash,
        value_in: u64,
        value_out: u64,
    },
    /// Block reward (coinbase value) exceeds the allowed subsidy + transaction fees.
    #[error("Block#{0} reward exceeds allowed amount (subsidy + fees)")]
    InvalidBlockReward(BlockHash),
    /// A transaction script failed verification.
    #[error(
        "Script verification failure in block#{block_hash}. Context: {context}, input_index: {input_index}: {error:?}"
    )]
    InvalidScript {
        block_hash: BlockHash,
        context: TransactionContext,
        input_index: usize,
        error: Box<subcoin_script::Error>,
    },
    #[error("BIP34 error in block#{0}: {1:?}")]
    Bip34(BlockHash, Bip34Error),
    #[error("Bitcoin codec error: {0:?}")]
    BitcoinCodec(bitcoin::io::Error),
    #[error(transparent)]
    BitcoinConsensus(#[from] bitcoinconsensus::Error),
    /// An error occurred in the client.
    #[error(transparent)]
    Client(#[from] sp_blockchain::Error),
    #[error(transparent)]
    Transaction(#[from] TxError),
    /// Block header error.
    #[error(transparent)]
    Header(#[from] HeaderError),
}

/// A struct responsible for verifying Bitcoin blocks.
#[derive(Clone)]
pub struct BlockVerifier<Block, Client, BE> {
    client: Arc<Client>,
    chain_params: ChainParams,
    header_verifier: HeaderVerifier<Block, Client>,
    block_verification: BlockVerification,
    /// Bitcoin state for O(1) UTXO lookups.
    bitcoin_state: Arc<BitcoinState>,
    script_engine: ScriptEngine,
    /// Whether to use parallel script verification.
    parallel_verification: bool,
    _phantom: PhantomData<(Block, BE)>,
}

impl<Block, Client, BE> BlockVerifier<Block, Client, BE> {
    /// Constructs a new instance of [`BlockVerifier`].
    pub fn new(
        client: Arc<Client>,
        network: bitcoin::Network,
        block_verification: BlockVerification,
        bitcoin_state: Arc<BitcoinState>,
        script_engine: ScriptEngine,
        parallel_verification: bool,
    ) -> Self {
        let chain_params = ChainParams::new(network);
        let header_verifier = HeaderVerifier::new(client.clone(), chain_params.clone());
        Self {
            client,
            chain_params,
            header_verifier,
            block_verification,
            bitcoin_state,
            script_engine,
            parallel_verification,
            _phantom: Default::default(),
        }
    }
}

impl<Block, Client, BE> BlockVerifier<Block, Client, BE>
where
    Block: BlockT,
    BE: Backend<Block>,
    Client: HeaderBackend<Block> + StorageProvider<Block, BE> + AuxStore,
{
    /// Performs full block verification.
    ///
    /// Returns the script verification duration (zero if verification was skipped).
    ///
    /// References:
    /// - <https://en.bitcoin.it/wiki/Protocol_rules#.22block.22_messages>
    pub fn verify_block(
        &self,
        block_number: u32,
        block: &BitcoinBlock,
    ) -> Result<std::time::Duration, Error> {
        let txids = self.check_block_sanity(block_number, block)?;

        self.contextual_check_block(block_number, block.block_hash(), block, txids)
    }

    fn contextual_check_block(
        &self,
        block_number: u32,
        block_hash: BlockHash,
        block: &BitcoinBlock,
        txids: HashMap<usize, Txid>,
    ) -> Result<std::time::Duration, Error> {
        match self.block_verification {
            BlockVerification::Full => {
                let lock_time_cutoff = self.header_verifier.verify(&block.header)?;

                if block_number >= self.chain_params.segwit_height
                    && !block.check_witness_commitment()
                {
                    return Err(Error::BadWitnessCommitment(block_hash));
                }

                // Check the block weight with witness data.
                if block.weight() > MAX_BLOCK_WEIGHT {
                    return Err(Error::BlockTooLarge(block_hash));
                }

                self.verify_transactions(block_number, block, txids, lock_time_cutoff)
            }
            BlockVerification::HeaderOnly => {
                self.header_verifier.verify(&block.header)?;
                Ok(std::time::Duration::ZERO)
            }
            BlockVerification::None => Ok(std::time::Duration::ZERO),
        }
    }

    /// Performs preliminary checks.
    ///
    /// - Transaction list must be non-empty.
    /// - Block size must not exceed [`MAX_BLOCK_WEIGHT`].
    /// - First transaction must be coinbase, the rest must not be.
    /// - No duplicate transactions in the block.
    /// - Check the sum of transaction sig opcounts does not exceed [`MAX_BLOCK_SIGOPS_COST`].
    /// - Check the calculated merkle root of transactions matches the one declared in the header.
    ///
    /// <https://github.com/bitcoin/bitcoin/blob/6f9db1ebcab4064065ccdCOIN787161bf2b87e03cc1f/src/validation.cpp#L3986>
    fn check_block_sanity(
        &self,
        block_number: u32,
        block: &BitcoinBlock,
    ) -> Result<HashMap<usize, Txid>, Error> {
        let block_hash = block.block_hash();

        if block.txdata.is_empty() {
            return Err(Error::EmptyTransactionList(block_hash));
        }

        // Size limits, without tx witness data.
        if Weight::from_wu((block.txdata.len() * WITNESS_SCALE_FACTOR) as u64) > MAX_BLOCK_WEIGHT
            || Weight::from_wu((block_base_size(block) * WITNESS_SCALE_FACTOR) as u64)
                > MAX_BLOCK_WEIGHT
        {
            return Err(Error::BlockTooLarge(block_hash));
        }

        if !block.txdata[0].is_coinbase() {
            return Err(Error::FirstTransactionIsNotCoinbase(block_hash));
        }

        // Check duplicate transactions
        let tx_count = block.txdata.len();

        let mut seen_transactions = HashSet::with_capacity(tx_count);
        let mut txids = HashMap::with_capacity(tx_count);

        let mut sig_ops = 0;

        for (index, tx) in block.txdata.iter().enumerate() {
            if index > 0 && tx.is_coinbase() {
                return Err(Error::MultipleCoinbase(block_hash));
            }

            let txid = tx.compute_txid();
            if !seen_transactions.insert(txid) {
                // If txid is already in the set, we've found a duplicate.
                return Err(Error::DuplicateTransaction { block_hash, index });
            }

            check_transaction_sanity(tx)?;

            sig_ops += get_legacy_sig_op_count(tx);

            txids.insert(index, txid);
        }

        if sig_ops * WITNESS_SCALE_FACTOR > MAX_BLOCK_SIGOPS_COST as usize {
            return Err(Error::TooManySigOps {
                block_hash,
                block_number,
            });
        }

        // Inline `Block::check_merkle_root()` to avoid redundantly computing txid.
        let hashes = block
            .txdata
            .iter()
            .enumerate()
            .filter_map(|(index, _obj)| txids.get(&index).map(|txid| txid.to_raw_hash()));

        let maybe_merkle_root: Option<TxMerkleNode> =
            bitcoin::merkle_tree::calculate_root(hashes).map(|h| h.into());

        if !maybe_merkle_root
            .map(|merkle_root| block.header.merkle_root == merkle_root)
            .unwrap_or(false)
        {
            return Err(Error::BadMerkleRoot(block_hash));
        }

        Ok(txids)
    }

    /// Verifies all transactions in the block using a two-phase approach.
    ///
    /// Phase 1 (Sequential): UTXO validation, double-spend checks, and task collection.
    /// Phase 2 (Parallel or Sequential): Script verification.
    ///
    /// Returns the duration spent on script verification for benchmarking.
    fn verify_transactions(
        &self,
        block_number: u32,
        block: &BitcoinBlock,
        txids: HashMap<usize, Txid>,
        lock_time_cutoff: u32,
    ) -> Result<std::time::Duration, Error> {
        let parent_number = block_number - 1;
        let parent_hash =
            self.client
                .hash(parent_number.into())?
                .ok_or(sp_blockchain::Error::Backend(format!(
                    "Parent block #{parent_number} not found"
                )))?;

        let get_txid = |tx_index: usize| {
            txids
                .get(&tx_index)
                .copied()
                .expect("Txid must exist as initialized in `check_block_sanity()`; qed")
        };

        let tx_context = |tx_index| TransactionContext {
            block_number,
            tx_index,
            txid: get_txid(tx_index),
        };

        let flags = script_verify::get_block_script_flags(
            block_number,
            block.block_hash(),
            &self.chain_params,
        );

        let block_hash = block.block_hash();

        let mut block_fee = 0;
        let mut spent_coins_in_block = HashSet::new();

        // Estimate input count for pre-allocation (average ~2 inputs per non-coinbase tx).
        let estimated_inputs = block.txdata.len().saturating_sub(1).saturating_mul(2);
        let mut script_tasks: Vec<ScriptVerificationTask> = Vec::with_capacity(estimated_inputs);

        let use_core_engine = matches!(self.script_engine, ScriptEngine::Core);

        // ========== Phase 1: Sequential UTXO validation + collect script tasks ==========
        for (tx_index, tx) in block.txdata.iter().enumerate() {
            if tx_index == 0 {
                // Enforce rule that the coinbase starts with serialized block height.
                if block_number >= self.chain_params.params.bip34_height {
                    let block_height_in_coinbase = block
                        .bip34_block_height()
                        .map_err(|err| Error::Bip34(block_hash, err))?
                        as u32;
                    if block_height_in_coinbase != block_number {
                        return Err(Error::BadCoinbaseBlockHeight {
                            block_hash,
                            expected: block_number,
                            got: block_height_in_coinbase,
                        });
                    }
                }

                continue;
            }

            if !is_final_tx(tx, block_number, lock_time_cutoff) {
                return Err(Error::TransactionNotFinal(block_hash));
            }

            // Serialize transaction once for Core engine.
            let spending_tx_bytes = if use_core_engine {
                let mut buffer = Vec::with_capacity(tx.total_size());
                tx.consensus_encode(&mut buffer)
                    .map_err(Error::BitcoinCodec)?;
                buffer
            } else {
                Vec::new()
            };

            let access_coin = |out_point: OutPoint| -> Result<(TxOut, bool, u32), Error> {
                let maybe_coin = match self.find_utxo_in_state(parent_hash, out_point) {
                    Some(coin) => {
                        let Coin {
                            is_coinbase,
                            amount,
                            height,
                            script_pubkey,
                        } = coin;

                        let txout = TxOut {
                            value: Amount::from_sat(amount),
                            script_pubkey: ScriptBuf::from_bytes(script_pubkey),
                        };

                        Some((txout, is_coinbase, height))
                    }
                    None => find_utxo_in_current_block(block, out_point, tx_index, get_txid)
                        .map(|(txout, is_coinbase)| (txout, is_coinbase, block_number)),
                };

                maybe_coin.ok_or_else(|| Error::MissingUtxoInState {
                    block_hash,
                    context: tx_context(tx_index),
                    missing_utxo: out_point,
                })
            };

            // CheckTxInputs.
            let mut value_in = 0;
            let mut sig_ops_cost = 0;

            for (input_index, input) in tx.input.iter().enumerate() {
                let coin = input.previous_output;

                // Double-spend check (must remain sequential).
                if spent_coins_in_block.contains(&coin) {
                    return Err(Error::AlreadySpentInCurrentBlock {
                        block_hash,
                        block_number,
                        txid: get_txid(tx_index),
                        utxo: coin,
                    });
                }

                // Access coin (O(1) RocksDB lookup or in-block search).
                let (spent_output, is_coinbase, coin_height) = access_coin(coin)?;

                // If coin is coinbase, check that it's matured.
                if is_coinbase && block_number - coin_height < COINBASE_MATURITY {
                    return Err(Error::PrematureSpendOfCoinbase(block_hash));
                }

                // Collect script verification task (instead of verifying inline).
                if !matches!(self.script_engine, ScriptEngine::None) {
                    let task = ScriptVerificationTask {
                        spending_tx_bytes: spending_tx_bytes.clone(),
                        tx_context: tx_context(tx_index),
                        input_index,
                        spent_output: spent_output.clone(),
                        flags,
                        tx: if use_core_engine {
                            None
                        } else {
                            Some(tx.clone())
                        },
                    };
                    script_tasks.push(task);
                }

                spent_coins_in_block.insert(coin);
                value_in += spent_output.value.to_sat();
            }

            // > GetTransactionSigOpCost counts 3 types of sigops:
            // > * legacy (always)
            // > * p2sh (when P2SH enabled in flags and excludes coinbase)
            // > * witness (when witness enabled in flags and excludes coinbase)
            sig_ops_cost += tx.total_sigop_cost(|out_point: &OutPoint| {
                access_coin(*out_point).map(|(txout, _, _)| txout).ok()
            });

            if sig_ops_cost > MAX_BLOCK_SIGOPS_COST as usize {
                return Err(Error::TooManySigOps {
                    block_hash,
                    block_number,
                });
            }

            let value_out = tx
                .output
                .iter()
                .map(|output| output.value.to_sat())
                .sum::<u64>();

            // Total input value must be no less than total output value.
            // Tx fee is the difference between inputs and outputs.
            let tx_fee = value_in
                .checked_sub(value_out)
                .ok_or(Error::InsufficientFunds {
                    block_hash,
                    value_in,
                    value_out,
                })?;

            block_fee += tx_fee;
        }

        // ========== Phase 2: Script verification (parallel or sequential) ==========
        let verify_start = std::time::Instant::now();

        if self.parallel_verification {
            verify_scripts_parallel(&script_tasks, self.script_engine, block_hash)?;
        } else {
            verify_scripts_sequential(&script_tasks, self.script_engine, block_hash)?;
        }

        let verify_scripts_duration = verify_start.elapsed();

        // Validate block reward.
        let coinbase_value = block.txdata[0]
            .output
            .iter()
            .map(|output| output.value.to_sat())
            .sum::<u64>();

        let subsidy = bitcoin_block_subsidy(block_number);

        // Ensures no inflation.
        if coinbase_value > block_fee + subsidy {
            return Err(Error::InvalidBlockReward(block_hash));
        }

        Ok(verify_scripts_duration)
    }

    /// Finds a UTXO in Bitcoin state (O(1) lookup via RocksDB).
    fn find_utxo_in_state(&self, _block_hash: Block::Hash, out_point: OutPoint) -> Option<Coin> {
        self.bitcoin_state.get(&out_point).map(|state_coin| {
            // Convert state Coin to runtime Coin (same structure)
            Coin {
                is_coinbase: state_coin.is_coinbase,
                amount: state_coin.amount,
                height: state_coin.height,
                script_pubkey: state_coin.script_pubkey,
            }
        })
    }
}

// Find a UTXO from the previous transactions in current block.
fn find_utxo_in_current_block(
    block: &BitcoinBlock,
    out_point: OutPoint,
    tx_index: usize,
    get_txid: impl Fn(usize) -> Txid,
) -> Option<(TxOut, bool)> {
    let OutPoint { txid, vout } = out_point;
    block
        .txdata
        .iter()
        .take(tx_index)
        .enumerate()
        .find_map(|(index, tx)| (get_txid(index) == txid).then_some((tx, index == 0)))
        .and_then(|(tx, is_coinbase)| {
            tx.output
                .get(vout as usize)
                .cloned()
                .map(|txout| (txout, is_coinbase))
        })
}

/// Returns the base block size.
///
/// > Base size is the block size in bytes with the original transaction serialization without
/// > any witness-related data, as seen by a non-upgraded node.
// TODO: copied from rust-bitcoin, send a patch upstream to make this API public?
fn block_base_size(block: &BitcoinBlock) -> usize {
    let mut size = BitcoinHeader::SIZE;

    size += VarInt::from(block.txdata.len()).size();
    size += block.txdata.iter().map(|tx| tx.base_size()).sum::<usize>();

    size
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::consensus::encode::deserialize_hex;

    #[test]
    fn test_find_utxo_in_current_block() {
        let test_block = std::env::current_dir()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("test_data")
            .join("btc_mainnet_385044.data");
        let raw_block = std::fs::read_to_string(test_block).unwrap();
        let block = deserialize_hex::<BitcoinBlock>(raw_block.trim()).unwrap();

        let txids = block
            .txdata
            .iter()
            .enumerate()
            .map(|(index, tx)| (index, tx.compute_txid()))
            .collect::<HashMap<_, _>>();

        // 385044:35:1
        let out_point = OutPoint {
            txid: "2b102a19161e5c93f71e16f9e8c9b2438f362c51ecc8f2a62e3c31d7615dd17d"
                .parse()
                .unwrap(),
            vout: 1,
        };

        // The input of block 385044:36 is from the previous transaction 385044:35:1.
        // https://www.blockchain.com/explorer/transactions/btc/5645cb0a3953b7766836919566b25321a976d06c958e69ff270358233a8c82d6
        assert_eq!(
            find_utxo_in_current_block(&block, out_point, 36, |index| txids
                .get(&index)
                .copied()
                .unwrap())
            .map(|(txout, is_coinbase)| (txout.value.to_sat(), is_coinbase))
            .unwrap(),
            (295600000, false)
        );
    }
}
