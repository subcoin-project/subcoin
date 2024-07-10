use bitcoin::blockdata::block::{Header as BitcoinHeader, ValidationError};
use bitcoin::consensus::Params;
use bitcoin::pow::U256;
use bitcoin::{Block as BitcoinBlock, OutPoint, Target, Txid};
use sc_client_api::{AuxStore, Backend, StorageProvider};
use sp_blockchain::HeaderBackend;
use sp_runtime::traits::Block as BlockT;
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use subcoin_primitives::runtime::bitcoin_block_subsidy;
use subcoin_primitives::{BackendExt, CoinStorageKey};

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
    /// Block must contain at least one coinbase transaction.
    #[error("No coinbase transaction")]
    NoCoinbase,
    #[error("Bad merkle root")]
    BadMerkleRoot,
    #[error("Block time is too far in future")]
    TooFarInFuture,
    #[error("Invalid header: {0:?}")]
    InvalidHeader(ValidationError),
    #[error(transparent)]
    Client(#[from] sp_blockchain::Error),
    #[error("Not enough PoW")]
    NotEnoughPow,
    ///////////////////////////////////
    // Transaction verification error.
    ///////////////////////////////////
    #[error("Coinbase must be the first transaction")]
    FirstTxIsNotCoinbase,
    #[error("Referenced output in block {0} not found: {1:?}")]
    OutputNotFound(u32, OutPoint),
    #[error("Amount of total outputs is larger than the amount of total inputs")]
    NotEnoughMoney,
    #[error("Block reward is larger than the sum of block fee and subsidy")]
    InvalidBlockReward,
}

#[derive(Clone)]
pub struct BlockVerifier<Block, Client, BE> {
    client: Arc<Client>,
    block_verification: BlockVerification,
    consensus_params: Params,
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
        Self {
            client,
            block_verification,
            consensus_params: Params::new(network),
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
    pub fn verify_block(&self, block_number: u32, block: &BitcoinBlock) -> Result<(), Error> {
        match self.block_verification {
            BlockVerification::Full => {
                self.check_block_sanity(block)?;
                self.verify_block_header(&block.header)?;
                self.verify_transactions(block_number, block)?;
            }
            BlockVerification::HeaderOnly => {
                self.check_block_sanity(block)?;
                self.verify_block_header(&block.header)?;
            }
            BlockVerification::None => {}
        }

        Ok(())
    }

    /// Pre-verification.
    fn check_block_sanity(&self, block: &BitcoinBlock) -> Result<(), Error> {
        if block.coinbase().is_none() {
            return Err(Error::NoCoinbase);
        }

        if !block.check_merkle_root() {
            return Err(Error::BadMerkleRoot);
        }

        // Once segwit is active, we will still need to check for block mutability.

        Ok(())
    }

    /// Verifies the validity of header.
    pub fn verify_block_header(&self, header: &BitcoinHeader) -> Result<(), Error> {
        // Check proof of work.
        let expected_target = self.get_next_work_required(header);

        let actual_target = header.target();

        if actual_target > expected_target {
            return Err(Error::NotEnoughPow);
        }

        header
            .validate_pow(actual_target)
            .map_err(Error::InvalidHeader)?;

        const MAX_FUTURE_BLOCK_TIME: u32 = 2 * 60 * 60; // 2 hours

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

    /// Return the bits of `next_header`.
    ///
    /// Usually, it's just the target of last block. However, if we are in a retarget period,
    /// it will be calculated from the last 2016 blocks (about two weeks for Bitcoin mainnet).
    ///
    /// https://github.com/bitcoin/bitcoin/blob/89b910711c004c21b7d67baa888073742f7f94f0/src/pow.cpp#L13
    fn get_next_work_required(&self, next_header: &BitcoinHeader) -> Target {
        let last_block = self
            .client
            .block_header(next_header.prev_blockhash)
            .expect("Parent block must exist as we checked before; qed");

        if self.consensus_params.no_pow_retargeting {
            return last_block.target();
        }

        let last_block_height = self
            .client
            .block_number(last_block.block_hash())
            .expect("Parent block must exist as we checked before; qed");

        let height = last_block_height + 1;

        let difficulty_adjustment_interval =
            self.consensus_params.difficulty_adjustment_interval() as u32;

        if height >= difficulty_adjustment_interval && height % difficulty_adjustment_interval == 0
        {
            let last_retarget_height = height - difficulty_adjustment_interval;

            let retarget_header_hash = self
                .client
                .block_hash(last_retarget_height)
                .expect("Retarget block must be available; qed");

            let retarget_header = self
                .client
                .block_header(retarget_header_hash)
                .expect("Retarget block must be available; qed");

            let first_block_time = retarget_header.time;

            // timestamp of last block
            let last_block_time = last_block.time;

            self.calculate_next_work_required(
                retarget_header.target().0,
                first_block_time.into(),
                last_block_time.into(),
            )
        } else {
            last_block.target()
        }
    }

    // https://github.com/bitcoin/bitcoin/blob/89b910711c004c21b7d67baa888073742f7f94f0/src/pow.cpp#L49-L72
    fn calculate_next_work_required(
        &self,
        previous_target: U256,
        first_block_time: u64,
        last_block_time: u64,
    ) -> Target {
        let mut actual_timespan = last_block_time.saturating_sub(first_block_time);

        let pow_target_timespan = self.consensus_params.pow_target_timespan;

        // Limit adjustment step.
        if actual_timespan < pow_target_timespan / 4 {
            actual_timespan = pow_target_timespan / 4;
        }

        if actual_timespan > pow_target_timespan * 4 {
            actual_timespan = pow_target_timespan * 4;
        }

        let pow_limit = self.consensus_params.max_attainable_target;

        // Retarget.
        let target = previous_target * actual_timespan.into();
        let target = Target(target / pow_target_timespan.into());

        if target > pow_limit {
            pow_limit
        } else {
            target
        }
    }

    fn verify_transactions(&self, block_number: u32, block: &BitcoinBlock) -> Result<(), Error> {
        let transactions = &block.txdata;

        let parent_hash =
            self.client
                .hash((block_number - 1).into())?
                .ok_or(sp_blockchain::Error::Backend(format!(
                    "Parent block #{} not found",
                    block_number - 1
                )))?;

        let mut block_fee = 0;

        // TODO: verify transactions in parallel.
        for (index, tx) in transactions.iter().enumerate() {
            if tx.is_coinbase() {
                if index == 0 {
                    continue;
                } else {
                    return Err(Error::FirstTxIsNotCoinbase);
                }
            }

            let output_value = tx
                .output
                .iter()
                .map(|output| output.value.to_sat())
                .sum::<u64>();

            let mut input_value = 0;

            for input in &tx.input {
                let OutPoint { txid, vout } = input.previous_output;

                let amount = match self.fetch_coin_at(parent_hash, txid, vout) {
                    Some(coin) => coin.amount,
                    None => fetch_coin_value_in_current_block(block, txid, vout, index)
                        .ok_or(Error::OutputNotFound(block_number, input.previous_output))?,
                };

                input_value += amount;
            }

            if output_value > input_value {
                return Err(Error::NotEnoughMoney);
            }

            // Tx fee is the difference between inputs and outputs.
            let tx_fee = input_value - output_value;

            block_fee += tx_fee;

            // TODO: Verify the script.
        }

        let block_reward = transactions[0]
            .output
            .iter()
            .map(|output| output.value.to_sat())
            .sum::<u64>();

        let subsidy = bitcoin_block_subsidy(block_number);

        // Ensures no inflation.
        if block_reward > block_fee + subsidy {
            return Err(Error::InvalidBlockReward);
        }

        Ok(())
    }

    fn fetch_coin_at(&self, block_hash: Block::Hash, txid: Txid, index: u32) -> Option<Coin> {
        use codec::Decode;

        // Read state from the backend
        //
        // TODO: read the state from the in memory backend if available?
        let storage_key = self.coin_storage_key.storage_key(txid, index);

        let maybe_storage_data = self
            .client
            .storage(block_hash, &sc_client_api::StorageKey(storage_key))
            .ok()
            .flatten();

        maybe_storage_data.and_then(|data| Coin::decode(&mut data.0.as_slice()).ok())
    }
}

/// UTXO.
#[derive(Debug, codec::Encode, codec::Decode)]
pub struct Coin {
    /// Whether the coin is from a coinbase transaction.
    pub is_coinbase: bool,
    /// Transfer value in satoshis.
    pub amount: u64,
    /// Spending condition of the output.
    pub script_pubkey: Vec<u8>,
}

fn fetch_coin_value_in_current_block(
    block: &BitcoinBlock,
    txid: Txid,
    vout: u32,
    transaction_index: usize,
) -> Option<u64> {
    let take = transaction_index.min(block.txdata.len());
    let transactions = &block.txdata[..take];
    transactions
        .iter()
        .find(|tx| tx.compute_txid() == txid)
        .and_then(|tx| tx.output.get(vout as usize))
        .map(|txout| txout.value.to_sat())
}
