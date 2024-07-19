use bitcoin::blockdata::block::{Header as BitcoinHeader, ValidationError};
use bitcoin::consensus::Params;
use bitcoin::hashes::Hash;
use bitcoin::pow::U256;
use bitcoin::{BlockHash, Target};
use sc_client_api::AuxStore;
use sp_blockchain::HeaderBackend;
use sp_runtime::traits::Block as BlockT;
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use subcoin_primitives::BackendExt;

// 2 hours
const MAX_FUTURE_BLOCK_TIME: u32 = 2 * 60 * 60;

/// Block header error.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Block's difficulty is invalid.
    #[error("Incorrect proof-of-work")]
    BadDifficultyBits,
    /// Block's proof-of-work is invalid.
    #[error("proof-of-work validation failed: {0:?}")]
    InvalidProofOfWork(ValidationError),
    /// Block's timestamp is too far in the future.
    #[error("Block time is too far in the future")]
    TooFarInFuture,
    /// Block's timestamp is too old.
    #[error("Time is the median time of last 11 blocks or before")]
    TimeTooOld,
    /// An error occurred in the client.
    #[error(transparent)]
    Client(#[from] sp_blockchain::Error),
}

/// A struct responsible for verifying block header.
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
    /// Verifies the validity of header and returns the block time for verifying the finality of
    /// transactions.
    ///
    /// - Check proof of work.
    /// - Check the timestamp of block.
    ///     - Time is not greater than 2 hours from now.
    ///     - Time is not the median time of last 11 blocks or before.
    ///
    /// https://github.com/bitcoin/bitcoin/blob/6f9db1ebcab4064065ccd787161bf2b87e03cc1f/src/validation.cpp#L4146
    pub fn verify_header(&self, header: &BitcoinHeader) -> Result<u32, Error> {
        let prev_block_hash = header.prev_blockhash;

        let prev_block_header = self.client.block_header(prev_block_hash).ok_or(
            sp_blockchain::Error::MissingHeader(prev_block_hash.to_string()),
        )?;

        let prev_block_height = self
            .client
            .block_number(prev_block_hash)
            .expect("Prev block must exist as we checked before; qed");

        let expected_target = get_next_work_required(
            prev_block_height,
            prev_block_header,
            &self.consensus_params,
            &self.client,
        );

        let actual_target = header.target();

        if actual_target != expected_target {
            return Err(Error::BadDifficultyBits);
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

        let block_number = prev_block_height + 1;

        // TODO: check deployment state properly.
        const MAINNET_CSV_HEIGHT: u32 = 419328;

        if block_number >= MAINNET_CSV_HEIGHT {
            let mtp = self.calculate_median_time_past(header);
            if header.time <= mtp {
                return Err(Error::TimeTooOld);
            }

            Ok(mtp)
        } else {
            Ok(header.time)
        }
    }

    /// Calculates the median time of the previous few blocks prior to the header (inclusive).
    fn calculate_median_time_past(&self, header: &BitcoinHeader) -> u32 {
        const LAST_BLOCKS: usize = 11;

        let mut timestamps = Vec::with_capacity(LAST_BLOCKS);

        timestamps.push(header.time);

        let zero_hash = BlockHash::all_zeros();

        let mut block_hash = header.prev_blockhash;

        for _ in 0..LAST_BLOCKS - 1 {
            // Genesis block
            if block_hash == zero_hash {
                break;
            }

            let header = self
                .client
                .block_header(block_hash)
                .expect("Parent header must exist; qed");

            timestamps.push(header.time);

            block_hash = header.prev_blockhash;
        }

        timestamps.sort_unstable();

        timestamps
            .get(timestamps.len() / 2)
            .copied()
            .expect("Timestamps must be non-empty; qed")
    }
}

/// Usually, it's just the target of last block. However, if we are in a retarget period,
/// it will be calculated from the last 2016 blocks (about two weeks for Bitcoin mainnet).
///
/// https://github.com/bitcoin/bitcoin/blob/89b910711c004c21b7d67baa888073742f7f94f0/src/pow.cpp#L13
fn get_next_work_required<Block, Client>(
    last_block_height: u32,
    last_block: BitcoinHeader,
    consensus_params: &Params,
    client: &Arc<Client>,
) -> Target
where
    Block: BlockT,
    Client: HeaderBackend<Block> + AuxStore,
{
    if consensus_params.no_pow_retargeting {
        return last_block.target();
    }

    let height = last_block_height + 1;

    let difficulty_adjustment_interval = consensus_params.difficulty_adjustment_interval() as u32;

    // Only change once per difficulty adjustment interval.
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
