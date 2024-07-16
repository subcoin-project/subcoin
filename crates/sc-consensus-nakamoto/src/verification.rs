use bitcoin::blockdata::block::{Header as BitcoinHeader, ValidationError};
use bitcoin::consensus::Params;
use bitcoin::pow::U256;
use bitcoin::{Block as BitcoinBlock, OutPoint, Target, Transaction, TxMerkleNode, Txid};
use sc_client_api::{AuxStore, Backend, StorageProvider};
use sp_blockchain::HeaderBackend;
use sp_runtime::traits::Block as BlockT;
use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use subcoin_primitives::runtime::{bitcoin_block_subsidy, Coin};
use subcoin_primitives::{BackendExt, CoinStorageKey};

// 2 hours
const MAX_FUTURE_BLOCK_TIME: u32 = 2 * 60 * 60;

// MinCoinbaseScriptLen is the minimum length a coinbase script can be.
const MIN_COINBASE_SCRIPT_LEN: usize = 2;

// MaxCoinbaseScriptLen is the maximum length a coinbase script can be.
const MAX_COINBASE_SCRIPT_LEN: usize = 100;

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
    /// The block's timestamp is too far in the future.
    #[error("Block time is too far in the future")]
    TooFarInFuture,
    /// The block header is invalid.
    #[error("proof-of-work validation failed: {0:?}")]
    InvalidProofOfWork(ValidationError),
    /// The block does not have enough proof-of-work.
    #[error("Insufficient proof-of-work")]
    NotEnoughPow,
    /// Block must contain at least one coinbase transaction.
    #[error("Transaction list is empty")]
    EmptyTransactionList,
    #[error("Transaction has no inputs")]
    EmptyInput,
    #[error("Transaction has no outputs")]
    EmptyOutput,
    #[error("First transaction is not coinbase")]
    FirstTransactionIsNotCoinbase,
    #[error("Block contains multiple coinbase transactions")]
    MultipleCoinbase,
    #[error("Block contains duplicate transaction at index {0}")]
    DuplicateTransaction(usize),
    #[error("Transaction contains duplicate inputs at index {0}")]
    DuplicateTxInputs(usize),
    #[error(
        "Coinbase transaction script length of {got} is out of range (min: {min}, max: {max})"
    )]
    BadScriptSigLength { got: usize, min: usize, max: usize },
    #[error("Transaction input refers to previous output that is null")]
    BadTxInput,
    #[error("UTXO spent in #{block_number}:{txid} not found: {output:?}")]
    UtxoNotFound {
        block_number: u32,
        txid: Txid,
        output: OutPoint,
    },
    #[error("Total output amount exceeds total input amount")]
    InsufficientFunds,
    #[error("Block reward is larger than the sum of block fee and subsidy")]
    InvalidBlockReward,
    /// An error occurred in the client.
    #[error(transparent)]
    Client(#[from] sp_blockchain::Error),
}

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
    /// Full block verification.
    ///
    /// References:
    /// - https://en.bitcoin.it/wiki/Protocol_rules#.22block.22_messages
    pub fn verify_block(&self, block_number: u32, block: &BitcoinBlock) -> Result<(), Error> {
        let txids = self.check_block_sanity(block)?;

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

    /// Performs some context free preliminary checks.
    fn check_block_sanity(&self, block: &BitcoinBlock) -> Result<HashMap<usize, Txid>, Error> {
        // Transaction list must be non-empty.
        if block.txdata.is_empty() {
            return Err(Error::EmptyTransactionList);
        }

        // First transaction must be coinbase, the rest must not be.
        if !block.txdata[0].is_coinbase() {
            return Err(Error::FirstTransactionIsNotCoinbase);
        }

        if block.txdata.iter().skip(1).any(|tx| tx.is_coinbase()) {
            return Err(Error::MultipleCoinbase);
        }

        // Check duplicate transactions
        let tx_count = block.txdata.len();
        let mut seen_transactions = HashSet::with_capacity(tx_count);
        let mut txids = HashMap::with_capacity(tx_count);
        for (index, tx) in block.txdata.iter().enumerate() {
            let txid = tx.compute_txid();
            if !seen_transactions.insert(txid) {
                // If txid is already in the set, we've found a duplicate.
                return Err(Error::DuplicateTransaction(index));
            }

            check_transaction_sanity(tx)?;

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

        // TODO: check max_sig_ops
        // ensure!(total_sig_ops <= MAX_BLOCK_SIGOPS_COST)

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
                let OutPoint { txid, vout } = input.previous_output;

                let amount = match self.fetch_utxo_in_state(parent_hash, txid, vout) {
                    Some(coin) => coin.amount,
                    None => {
                        let get_txid = |tx_index: usize| {
                            txids.get(&tx_index).copied().expect(
                                "Txid must exist as initialized in `check_block_sanity()`; qed",
                            )
                        };
                        // Try to find UTXO from the previous transactions in current block.
                        let take = tx_index.min(block.txdata.len());
                        let transactions = &block.txdata[..=take];
                        transactions
                            .iter()
                            .enumerate()
                            .find_map(|(index, tx)| (get_txid(index) == txid).then_some(tx))
                            .and_then(|tx| {
                                tx.output
                                    .get(vout as usize)
                                    .map(|txout| txout.value.to_sat())
                            })
                            .ok_or_else(|| Error::UtxoNotFound {
                                block_number,
                                txid: get_txid(tx_index),
                                output: input.previous_output,
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
        for (index, tx) in transactions.iter().skip(1).enumerate() {
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

    fn fetch_utxo_in_state(&self, block_hash: Block::Hash, txid: Txid, index: u32) -> Option<Coin> {
        use codec::Decode;

        // Read state from the backend
        //
        // TODO: optimizations:
        // - Read the state from the in memory backend.
        // - Maintain a flat in-memory UTXO cache and try to read from cache first.
        let storage_key = self.coin_storage_key.storage_key(txid, index);

        let maybe_storage_data = self
            .client
            .storage(block_hash, &sc_client_api::StorageKey(storage_key))
            .ok()
            .flatten();

        maybe_storage_data.and_then(|data| Coin::decode(&mut data.0.as_slice()).ok())
    }
}

fn check_transaction_sanity(tx: &Transaction) -> Result<(), Error> {
    if tx.input.is_empty() {
        return Err(Error::EmptyInput);
    }

    if tx.output.is_empty() {
        return Err(Error::EmptyOutput);
    }

    // Check for duplicate transaction inputs.
    let mut seen_inputs = HashSet::new();
    for (index, txin) in tx.input.iter().enumerate() {
        if !seen_inputs.insert(txin.previous_output) {
            return Err(Error::DuplicateTxInputs(index));
        }
    }

    // Coinbase script length must be between min and max length.
    if tx.is_coinbase() {
        let script_sig_len = tx.input[0].script_sig.len();

        if !(MIN_COINBASE_SCRIPT_LEN..=MAX_COINBASE_SCRIPT_LEN).contains(&script_sig_len) {
            return Err(Error::BadScriptSigLength {
                got: script_sig_len,
                min: MIN_COINBASE_SCRIPT_LEN,
                max: MAX_COINBASE_SCRIPT_LEN,
            });
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

#[derive(Clone)]
pub struct HeaderVerifier<Block, Client> {
    client: Arc<Client>,
    consensus_params: Params,
    _phantom: PhantomData<Block>,
}

impl<Block, Client> HeaderVerifier<Block, Client> {
    /// Constructs a new instance of [`HeaderVerifier`].
    pub fn new(client: Arc<Client>, consensus_params: Params) -> Self {
        Self {
            client,
            consensus_params,
            _phantom: Default::default(),
        }
    }
}

impl<Block, Client> HeaderVerifier<Block, Client>
where
    Block: BlockT,
    Client: HeaderBackend<Block> + AuxStore,
{
    /// Verifies the validity of header.
    ///
    /// - Check proof of work.
    /// - Check timestamp of the block .
    pub fn verify_header(&self, header: &BitcoinHeader) -> Result<(), Error> {
        let last_block_header = self.client.block_header(header.prev_blockhash).ok_or(
            sp_blockchain::Error::MissingHeader(header.prev_blockhash.to_string()),
        )?;

        let last_block_height = self
            .client
            .block_number(last_block_header.block_hash())
            .expect("Parent block must exist as we checked before; qed");

        let expected_target = get_next_work_required(
            last_block_height,
            last_block_header,
            &self.consensus_params,
            &self.client,
        );

        let actual_target = header.target();

        if actual_target > expected_target {
            return Err(Error::NotEnoughPow);
        }

        header
            .validate_pow(actual_target)
            .map_err(Error::InvalidProofOfWork)?;

        // Get the seconds since the UNIX epoch
        let current_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards")
            .as_secs() as u32;

        if header.time > current_time + MAX_FUTURE_BLOCK_TIME {
            return Err(Error::TooFarInFuture);
        }

        Ok(())
    }
}

/// Usually, it's just the target of last block. However, if we are in a retarget period,
/// it will be calculated from the last 2016 blocks (about two weeks for Bitcoin mainnet).
///
/// https://github.com/bitcoin/bitcoin/blob/89b910711c004c21b7d67baa888073742f7f94f0/src/pow.cpp#L13
fn get_next_work_required<Block: BlockT, Client: HeaderBackend<Block> + AuxStore>(
    last_block_height: u32,
    last_block: BitcoinHeader,
    consensus_params: &Params,
    client: &Arc<Client>,
) -> Target {
    if consensus_params.no_pow_retargeting {
        return last_block.target();
    }

    let height = last_block_height + 1;

    let difficulty_adjustment_interval = consensus_params.difficulty_adjustment_interval() as u32;

    if height >= difficulty_adjustment_interval && height % difficulty_adjustment_interval == 0 {
        let last_retarget_height = height - difficulty_adjustment_interval;

        let retarget_header_hash = client
            .block_hash(last_retarget_height)
            .expect("Retarget block must be available; qed");

        let retarget_header = client
            .block_header(retarget_header_hash)
            .expect("Retarget block must be available; qed");

        let first_block_time = retarget_header.time;

        // timestamp of last block
        let last_block_time = last_block.time;

        calculate_next_work_required(
            retarget_header.target().0,
            first_block_time.into(),
            last_block_time.into(),
            consensus_params,
        )
    } else {
        last_block.target()
    }
}

// https://github.com/bitcoin/bitcoin/blob/89b910711c004c21b7d67baa888073742f7f94f0/src/pow.cpp#L49-L72
fn calculate_next_work_required(
    previous_target: U256,
    first_block_time: u64,
    last_block_time: u64,
    consensus_params: &Params,
) -> Target {
    let mut actual_timespan = last_block_time.saturating_sub(first_block_time);

    let pow_target_timespan = consensus_params.pow_target_timespan;

    // Limit adjustment step.
    if actual_timespan < pow_target_timespan / 4 {
        actual_timespan = pow_target_timespan / 4;
    }

    if actual_timespan > pow_target_timespan * 4 {
        actual_timespan = pow_target_timespan * 4;
    }

    let pow_limit = consensus_params.max_attainable_target;

    // Retarget.
    let target = previous_target * actual_timespan.into();
    let target = Target(target / pow_target_timespan.into());

    if target > pow_limit {
        pow_limit
    } else {
        target
    }
}
