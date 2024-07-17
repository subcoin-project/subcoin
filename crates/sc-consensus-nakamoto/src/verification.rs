mod header_verifier;

use bitcoin::blockdata::constants::MAX_BLOCK_SIGOPS_COST;
use bitcoin::blockdata::weight::WITNESS_SCALE_FACTOR;
use bitcoin::consensus::Params;
use bitcoin::{Block as BitcoinBlock, OutPoint, Transaction, TxMerkleNode, Txid, VarInt, Weight};
use sc_client_api::{AuxStore, Backend, StorageProvider};
use sp_blockchain::HeaderBackend;
use sp_runtime::traits::Block as BlockT;
use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use std::sync::Arc;
use subcoin_primitives::runtime::{bitcoin_block_subsidy, Coin};
use subcoin_primitives::CoinStorageKey;

pub use header_verifier::{Error as HeaderError, HeaderVerifier};

// MinCoinbaseScriptLen is the minimum length a coinbase script can be.
const MIN_COINBASE_SCRIPT_LEN: usize = 2;

// MaxCoinbaseScriptLen is the maximum length a coinbase script can be.
const MAX_COINBASE_SCRIPT_LEN: usize = 100;

const MAX_BLOCK_WEIGHT: usize = Weight::MAX_BLOCK.to_wu() as usize;

/// Represents the level of block verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "cli", derive(clap::ValueEnum))]
pub enum BlockVerification {
    /// No verification performed.
    None,
    /// Full verification, including verifying the transactions.
    Full,
    /// Verify the block header only, without the transaction veification.
    HeaderOnly,
}

/// Block verification error.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The merkle root of the block is invalid.
    #[error("Invalid merkle root")]
    BadMerkleRoot,
    /// Block must contain at least one coinbase transaction.
    #[error("Transaction list is empty")]
    EmptyTransactionList,
    #[error("Block is too large")]
    BadBlockLength,
    #[error("Transaction has no inputs")]
    EmptyInput,
    #[error("Transaction has no outputs")]
    EmptyOutput,
    #[error("Transaction is too large")]
    BadTransactionLength,
    #[error("First transaction is not coinbase")]
    FirstTransactionIsNotCoinbase,
    #[error("Block contains multiple coinbase transactions")]
    MultipleCoinbase,
    #[error("Transaction script contains too many sigops (max: {MAX_BLOCK_SIGOPS_COST})")]
    TooManySigOps { block_number: u32, tx_index: usize },
    #[error("Block contains duplicate transaction at index {0}")]
    DuplicateTransaction(usize),
    #[error("Transaction contains duplicate inputs at index {0}")]
    DuplicateTxInput(usize),
    #[error(
        "Coinbase transaction script length of {0} is out of range \
        (min: {MIN_COINBASE_SCRIPT_LEN}, max: {MAX_COINBASE_SCRIPT_LEN})"
    )]
    BadScriptSigLength(usize),
    #[error("Transaction input refers to previous output that is null")]
    BadTxInput,
    /// Referenced output does not exist or has already been spent.
    #[error("UTXO spent in #{block_number}:{txid} not found: {out_point:?}")]
    UtxoNotFound {
        block_number: u32,
        txid: Txid,
        out_point: OutPoint,
    },
    #[error("Total output amount exceeds total input amount")]
    InsufficientFunds,
    // Invalid coinbase value.
    #[error("Block reward is larger than the sum of block fee and subsidy")]
    InvalidBlockReward,
    /// Block header error.
    #[error(transparent)]
    Header(#[from] HeaderError),
    /// An error occurred in the client.
    #[error(transparent)]
    Client(#[from] sp_blockchain::Error),
}

/// A struct responsible for verifying Bitcoin blocks.
#[derive(Clone)]
pub struct BlockVerifier<Block, Client, BE> {
    client: Arc<Client>,
    header_verifier: HeaderVerifier<Block, Client>,
    block_verification: BlockVerification,
    coin_storage_key: Arc<dyn CoinStorageKey>,
    _phantom: PhantomData<(Block, BE)>,
}

impl<Block, Client, BE> BlockVerifier<Block, Client, BE> {
    /// Constructs a new instance of [`BlockVerifier`].
    pub fn new(
        client: Arc<Client>,
        network: bitcoin::Network,
        block_verification: BlockVerification,
        coin_storage_key: Arc<dyn CoinStorageKey>,
    ) -> Self {
        let consensus_params = Params::new(network);
        let header_verifier = HeaderVerifier::new(client.clone(), consensus_params);
        Self {
            client,
            header_verifier,
            block_verification,
            coin_storage_key,
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

        match self.block_verification {
            BlockVerification::Full => {
                self.header_verifier.verify_header(&block.header)?;
                self.verify_transactions(block_number, block, txids)?;
            }
            BlockVerification::HeaderOnly => {
                self.header_verifier.verify_header(&block.header)?;
            }
            BlockVerification::None => {}
        }

        Ok(())
    }

    /// Performs context-free preliminary checks.
    ///
    /// - Transaction list must be non-empty.
    /// - Block size must not exceed `MAX_BLOCK_WEIGHT`.
    /// - First transaction must be coinbase, the rest must not be.
    /// - No duplicate transactions in the block.
    /// - Check the sum of transaction sig opcounts does not exceed [`MAX_BLOCK_SIGOPS_COST`].
    /// - Check the calculated merkle root of transactions matches the value declared in the header.
    ///
    /// https://github.com/bitcoin/bitcoin/blob/6f9db1ebcab4064065ccd787161bf2b87e03cc1f/src/validation.cpp#L3986
    fn check_block_sanity(
        &self,
        block_number: u32,
        block: &BitcoinBlock,
    ) -> Result<HashMap<usize, Txid>, Error> {
        if block.txdata.is_empty() {
            return Err(Error::EmptyTransactionList);
        }

        if !block.txdata[0].is_coinbase() {
            return Err(Error::FirstTransactionIsNotCoinbase);
        }

        // Size limits.
        let tx_count = block.txdata.len();

        if tx_count * WITNESS_SCALE_FACTOR > MAX_BLOCK_WEIGHT
            || block_serialize_size_no_witness(block) * WITNESS_SCALE_FACTOR > MAX_BLOCK_WEIGHT
        {
            return Err(Error::BadBlockLength);
        }

        // Check duplicate transactions
        let mut seen_transactions = HashSet::with_capacity(tx_count);
        let mut txids = HashMap::with_capacity(tx_count);

        for (index, tx) in block.txdata.iter().enumerate() {
            if index > 0 && tx.is_coinbase() {
                return Err(Error::MultipleCoinbase);
            }

            let txid = tx.compute_txid();
            if !seen_transactions.insert(txid) {
                // If txid is already in the set, we've found a duplicate.
                return Err(Error::DuplicateTransaction(index));
            }

            check_transaction_sanity(tx)?;

            if tx
                .input
                .iter()
                .any(|txin| txin.script_sig.count_sigops() > MAX_BLOCK_SIGOPS_COST as usize)
            {
                return Err(Error::TooManySigOps {
                    block_number,
                    tx_index: index,
                });
            }

            txids.insert(index, txid);
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
            return Err(Error::BadMerkleRoot);
        }

        // Once segwit is active, we will still need to check for block mutability.

        Ok(txids)
    }

    fn verify_transactions(
        &self,
        block_number: u32,
        block: &BitcoinBlock,
        txids: HashMap<usize, Txid>,
    ) -> Result<(), Error> {
        let transactions = &block.txdata;

        let parent_hash =
            self.client
                .hash((block_number - 1).into())?
                .ok_or(sp_blockchain::Error::Backend(format!(
                    "Parent block #{} not found",
                    block_number - 1
                )))?;

        // Verifies non-coinbase transaction.
        let verify_transaction = |tx_index: usize, tx: &Transaction| -> Result<u64, Error> {
            let total_output_value = tx
                .output
                .iter()
                .map(|output| output.value.to_sat())
                .sum::<u64>();

            let mut total_input_value = 0;

            for input in &tx.input {
                let out_point = input.previous_output;

                let amount = match self.find_utxo_in_state(parent_hash, out_point) {
                    Some(coin) => coin.amount,
                    None => {
                        let get_txid = |tx_index: usize| {
                            txids.get(&tx_index).copied().expect(
                                "Txid must exist as initialized in `check_block_sanity()`; qed",
                            )
                        };

                        find_utxo_in_current_block(block, out_point, tx_index, get_txid)
                            .ok_or_else(|| Error::UtxoNotFound {
                                block_number,
                                txid: get_txid(tx_index),
                                out_point,
                            })?
                    }
                };

                total_input_value += amount;
            }

            // Total input value must be no less than total output value.
            // Tx fee is the difference between inputs and outputs.
            let tx_fee = total_input_value
                .checked_sub(total_output_value)
                .ok_or(Error::InsufficientFunds)?;

            Ok(tx_fee)
        };

        let mut block_fee = 0;

        // TODO: verify transactions in parallel.
        for (index, tx) in transactions.iter().enumerate() {
            if index == 0 {
                // TODO: verify coinbase script
                continue;
            }

            let tx_fee = verify_transaction(index, tx)?;

            block_fee += tx_fee;

            // TODO: Verify the script.
        }

        let coinbase_value = transactions[0]
            .output
            .iter()
            .map(|output| output.value.to_sat())
            .sum::<u64>();

        let subsidy = bitcoin_block_subsidy(block_number);

        // Ensures no inflation.
        if coinbase_value > block_fee + subsidy {
            return Err(Error::InvalidBlockReward);
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

fn block_serialize_size_no_witness(block: &BitcoinBlock) -> usize {
    let base_size = 80 + VarInt(block.txdata.len() as u64).size();
    let tx_size: usize = block.txdata.iter().map(tx_serialize_size_no_witness).sum();
    base_size + tx_size
}

/// Returns the serialized transaction size without witness.
///
/// Credit: https://github.com/jrawsthorne/rust-bitcoin-node/blob/d84e4a63c4ae4d6818ab22bd0c25531d367961be/src/primitives/tx.rs#L287
fn tx_serialize_size_no_witness(tx: &Transaction) -> usize {
    // OutPoint (32+4)
    const OUTPOINT_SIZE: usize = 32 + 4;
    // Sequence (4)
    const SEQUENCE_SIZE: usize = 4;

    let input_size: usize = tx
        .input
        .iter()
        .map(|txin| {
            let script_sig_len = txin.script_sig.len();
            OUTPOINT_SIZE + SEQUENCE_SIZE + VarInt(script_sig_len as u64).size() + script_sig_len
        })
        .sum();

    // Amount (8)
    const VALUE_SIZE: usize = 8;
    let output_size: usize = tx
        .output
        .iter()
        .map(|txout| {
            let script_pubkey_len = txout.script_pubkey.len();
            VALUE_SIZE + VarInt(script_pubkey_len as u64).size() + script_pubkey_len
        })
        .sum();

    const VERSION_SIZE: usize = 4;
    const LOCK_TIME_SIZE: usize = 4;

    VERSION_SIZE
        + LOCK_TIME_SIZE
        + VarInt(tx.input.len() as u64).size() + input_size // Vec<TxIn>
        + VarInt(tx.output.len() as u64).size() + output_size // Vec<TxOut>
}

// Find a UTXO from the previous transactions in current block.
fn find_utxo_in_current_block(
    block: &BitcoinBlock,
    out_point: OutPoint,
    tx_index: usize,
    get_txid: impl Fn(usize) -> Txid,
) -> Option<u64> {
    let OutPoint { txid, vout } = out_point;
    block
        .txdata
        .iter()
        .take(tx_index)
        .enumerate()
        .find_map(|(index, tx)| (get_txid(index) == txid).then_some(tx))
        .and_then(|tx| {
            tx.output
                .get(vout as usize)
                .map(|txout| txout.value.to_sat())
        })
}

fn check_transaction_sanity(tx: &Transaction) -> Result<(), Error> {
    if tx.input.is_empty() {
        return Err(Error::EmptyInput);
    }

    if tx.output.is_empty() {
        return Err(Error::EmptyOutput);
    }

    if tx_serialize_size_no_witness(tx) * WITNESS_SCALE_FACTOR > MAX_BLOCK_WEIGHT {
        return Err(Error::BadTransactionLength);
    }

    // Check for duplicate inputs.
    let mut seen_inputs = HashSet::new();
    for (index, txin) in tx.input.iter().enumerate() {
        if !seen_inputs.insert(txin.previous_output) {
            return Err(Error::DuplicateTxInput(index));
        }
    }

    // Coinbase script length must be between min and max length.
    if tx.is_coinbase() {
        let script_sig_len = tx.input[0].script_sig.len();

        if !(MIN_COINBASE_SCRIPT_LEN..=MAX_COINBASE_SCRIPT_LEN).contains(&script_sig_len) {
            return Err(Error::BadScriptSigLength(script_sig_len));
        }
    } else {
        // Previous transaction outputs referenced by the inputs to this
        // transaction must not be null.
        if tx.input.iter().any(|txin| txin.previous_output.is_null()) {
            return Err(Error::BadTxInput);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::consensus::Decodable;
    use bitcoin::hex::FromHex;

    fn decode_raw_block(hex_str: &str) -> BitcoinBlock {
        let data = Vec::<u8>::from_hex(hex_str).expect("Failed to convert hex str");
        BitcoinBlock::consensus_decode(&mut data.as_slice())
            .expect("Failed to convert hex data to Block")
    }

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
        let block = decode_raw_block(raw_block.trim());

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
                .unwrap()),
            Some(295600000)
        );
    }
}
