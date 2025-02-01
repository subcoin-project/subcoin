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
use bitcoin::{
    Amount, Block as BitcoinBlock, BlockHash, OutPoint, ScriptBuf, TxMerkleNode, TxOut, Txid,
    VarInt, Weight,
};
use sc_client_api::{AuxStore, Backend, StorageProvider};
use sp_blockchain::HeaderBackend;
use sp_runtime::traits::Block as BlockT;
use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use std::sync::Arc;
use subcoin_primitives::runtime::{bitcoin_block_subsidy, Coin};
use subcoin_primitives::CoinStorageKey;
use tx_verify::{check_transaction_sanity, get_legacy_sig_op_count, is_final_tx};

pub use header_verify::{Error as HeaderError, HeaderVerifier};
pub use tx_verify::Error as TxError;

/// The maximum allowed weight for a block, see BIP 141 (network rule).
pub const MAX_BLOCK_WEIGHT: Weight = Weight::MAX_BLOCK;

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
#[derive(Debug)]
pub struct TransactionContext {
    /// Block number containing the transaction.
    pub block_number: u32,
    /// Index of the transaction in the block.
    pub tx_index: usize,
    /// ID of the transaction.
    pub txid: Txid,
}

/// Block verification error.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The merkle root of the block is invalid.
    #[error("Invalid merkle root in block#{0}")]
    BadMerkleRoot(BlockHash),
    /// Block must contain at least one coinbase transaction.
    #[error("Block {0} has an empty transaction list")]
    EmptyTransactionList(BlockHash),
    /// Block size exceeds the limit.
    #[error("Block {0} exceeds maximum allowed size")]
    BlockTooLarge(BlockHash),
    /// First transaction must be a coinbase.
    #[error("First transaction in block {0} is not a coinbase")]
    FirstTransactionIsNotCoinbase(BlockHash),
    /// A block must contain only one coinbase transaction.
    #[error("Block {0} contains multiple coinbase transactions")]
    MultipleCoinbase(BlockHash),
    #[error(
        "Block {block_hash} has incorrect coinbase height, (expected: {expected}, got: {got})"
    )]
    BadCoinbaseBlockHeight {
        block_hash: BlockHash,
        got: u32,
        expected: u32,
    },
    /// Coinbase transaction is prematurely spent.
    #[error("Premature spend of coinbase transaction in block {0}")]
    PrematureSpendOfCoinbase(BlockHash),

    /// Block contains duplicate transactions.
    #[error("Block {block_hash} contains duplicate transaction at index {index}")]
    DuplicateTransaction { block_hash: BlockHash, index: usize },
    /// A transaction is not finalized.
    #[error("Transaction in block {0} is not finalized")]
    TransactionNotFinal(BlockHash),
    /// Transaction script contains too many signature operations.
    #[error("Transaction in block #{block_number},{block_hash} exceeds signature operation limit (max: {MAX_BLOCK_SIGOPS_COST})")]
    TooManySigOps {
        block_hash: BlockHash,
        block_number: u32,
    },
    /// Witness commitment in the block is invalid.
    #[error("Invalid witness commitment in block {0}")]
    BadWitnessCommitment(BlockHash),

    /// UTXO referenced by a transaction is missing
    #[error(
        "Missing UTXO in state for transaction ({context:?}) in block {block_hash}. Missing UTXO: {missing_outpoint:?}"
    )]
    MissingUtxoInState {
        block_hash: BlockHash,
        /// Context of the transaction being processed.
        context: TransactionContext,
        /// UTXO missing from the UTXO set.
        missing_outpoint: OutPoint,
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
    #[error("Block {block_hash} has an invalid transaction: total input {value_in} < total output {value_out}")]
    InsufficientFunds {
        block_hash: BlockHash,
        value_in: u64,
        value_out: u64,
    },
    /// Block reward (coinbase value) exceeds the allowed subsidy + transaction fees.
    #[error("Block {0} reward exceeds allowed amount (subsidy + fees)")]
    InvalidBlockReward(BlockHash),
    /// A transaction script failed verification.
    #[error(
        "Script verification failure in block {block_hash}. Context: {context:?}, input_index: {input_index}: {error:?}"
    )]
    BadScript {
        block_hash: BlockHash,
        context: TransactionContext,
        input_index: usize,
        error: subcoin_script::Error,
    },
    #[error(transparent)]
    Transaction(#[from] TxError),
    /// Block header error.
    #[error(transparent)]
    Header(#[from] HeaderError),
    #[error(transparent)]
    BitcoinConsensus(#[from] bitcoinconsensus::Error),
    #[error("BIP34 error in block {0}: {1:?}")]
    Bip34(BlockHash, Bip34Error),
    #[error("Bitcoin codec error: {0:?}")]
    BitcoinCodec(bitcoin::io::Error),
    /// An error occurred in the client.
    #[error(transparent)]
    Client(#[from] sp_blockchain::Error),
}

/// A struct responsible for verifying Bitcoin blocks.
#[derive(Clone)]
pub struct BlockVerifier<Block, Client, BE> {
    client: Arc<Client>,
    chain_params: ChainParams,
    header_verifier: HeaderVerifier<Block, Client>,
    block_verification: BlockVerification,
    coin_storage_key: Arc<dyn CoinStorageKey>,
    script_engine: ScriptEngine,
    _phantom: PhantomData<(Block, BE)>,
}

impl<Block, Client, BE> BlockVerifier<Block, Client, BE> {
    /// Constructs a new instance of [`BlockVerifier`].
    pub fn new(
        client: Arc<Client>,
        network: bitcoin::Network,
        block_verification: BlockVerification,
        coin_storage_key: Arc<dyn CoinStorageKey>,
        script_engine: ScriptEngine,
    ) -> Self {
        let chain_params = ChainParams::new(network);
        let header_verifier = HeaderVerifier::new(client.clone(), chain_params.clone());
        Self {
            client,
            chain_params,
            header_verifier,
            block_verification,
            coin_storage_key,
            script_engine,
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
    /// References:
    /// - <https://en.bitcoin.it/wiki/Protocol_rules#.22block.22_messages>
    pub fn verify_block(&self, block_number: u32, block: &BitcoinBlock) -> Result<(), Error> {
        let txids = self.check_block_sanity(block_number, block)?;

        self.contextual_check_block(block_number, block.block_hash(), block, txids)
    }

    fn contextual_check_block(
        &self,
        block_number: u32,
        block_hash: BlockHash,
        block: &BitcoinBlock,
        txids: HashMap<usize, Txid>,
    ) -> Result<(), Error> {
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

                self.verify_transactions(block_number, block, txids, lock_time_cutoff)?;
            }
            BlockVerification::HeaderOnly => {
                self.header_verifier.verify(&block.header)?;
            }
            BlockVerification::None => {}
        }

        Ok(())
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

    fn verify_transactions(
        &self,
        block_number: u32,
        block: &BitcoinBlock,
        txids: HashMap<usize, Txid>,
        lock_time_cutoff: u32,
    ) -> Result<(), Error> {
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

        let mut tx_data = Vec::<u8>::new();

        // TODO: verify transactions in parallel.
        // https://github.com/bitcoin/bitcoin/blob/6f9db1ebcab4064065ccd787161bf2b87e03cc1f/src/validation.cpp#L2611
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

            tx_data.clear();
            tx.consensus_encode(&mut tx_data)
                .map_err(Error::BitcoinCodec)?;

            let spending_transaction = tx_data.as_slice();

            let access_coin = |out_point: OutPoint| -> Option<(TxOut, bool, u32)> {
                match self.find_utxo_in_state(parent_hash, out_point) {
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
                }
            };

            // CheckTxInputs.
            let mut value_in = 0;
            let mut sig_ops_cost = 0;

            for (input_index, input) in tx.input.iter().enumerate() {
                let coin = input.previous_output;

                if spent_coins_in_block.contains(&coin) {
                    return Err(Error::AlreadySpentInCurrentBlock {
                        block_hash,
                        block_number,
                        txid: get_txid(tx_index),
                        utxo: coin,
                    });
                }

                // Access coin.
                let (spent_output, is_coinbase, coin_height) =
                    access_coin(coin).ok_or_else(|| Error::MissingUtxoInState {
                        block_hash,
                        context: tx_context(tx_index),
                        missing_outpoint: coin,
                    })?;

                // If coin is coinbase, check that it's matured.
                if is_coinbase && block_number - coin_height < COINBASE_MATURITY {
                    return Err(Error::PrematureSpendOfCoinbase(block_hash));
                }

                match self.script_engine {
                    ScriptEngine::Core => {
                        script_verify::verify_input_script(
                            &spent_output,
                            spending_transaction,
                            input_index,
                            flags,
                        )?;
                    }
                    ScriptEngine::None => {
                        // Skip script verification.
                    }
                    ScriptEngine::Subcoin => {
                        let mut checker = subcoin_script::TransactionSignatureChecker::new(
                            input_index,
                            spent_output.value.to_sat(),
                            &tx,
                        );
                        let verify_flags =
                            subcoin_script::VerifyFlags::from_bits(flags).expect("Invalid flags");
                        subcoin_script::verify_script(
                            &input.script_sig,
                            &spent_output.script_pubkey,
                            &input.witness,
                            verify_flags,
                            &mut checker,
                        )
                        .map_err(|error| Error::BadScript {
                            block_hash,
                            context: tx_context(tx_index),
                            input_index,
                            error,
                        })?;
                    }
                }

                spent_coins_in_block.insert(coin);
                value_in += spent_output.value.to_sat();
            }

            // > GetTransactionSigOpCost counts 3 types of sigops:
            // > * legacy (always)
            // > * p2sh (when P2SH enabled in flags and excludes coinbase)
            // > * witness (when witness enabled in flags and excludes coinbase)
            sig_ops_cost += tx.total_sigop_cost(|out_point: &OutPoint| {
                access_coin(*out_point).map(|(txout, _, _)| txout)
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

        Ok(())
    }

    /// Finds a UTXO in the state backend.
    fn find_utxo_in_state(&self, block_hash: Block::Hash, out_point: OutPoint) -> Option<Coin> {
        use codec::Decode;

        // Read state from the backend
        //
        // TODO: optimizations:
        // - Read the state from the in memory backend.
        // - Maintain a flat in-memory UTXO cache and try to read from cache first.
        let OutPoint { txid, vout } = out_point;
        let storage_key = self.coin_storage_key.storage_key(txid, vout);

        let maybe_storage_data = self
            .client
            .storage(block_hash, &sc_client_api::StorageKey(storage_key))
            .ok()
            .flatten();

        maybe_storage_data.and_then(|data| Coin::decode(&mut data.0.as_slice()).ok())
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
