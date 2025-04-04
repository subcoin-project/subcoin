use crate::chain_params::{ChainParams, MEDIAN_TIME_SPAN};
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
    #[error("Incorrect proof-of-work: {{ got: {got:?}, expected: {expected:?} }}")]
    BadDifficultyBits { got: Target, expected: Target },
    /// Block's proof-of-work is invalid.
    #[error("proof-of-work validation failed: {0:?}")]
    InvalidProofOfWork(ValidationError),
    /// Block's timestamp is too far in the future.
    #[error("Block time is too far in the future")]
    TooFarInFuture,
    /// Block's timestamp is too old.
    #[error("Time is the median time of last 11 blocks or before")]
    TimeTooOld,
    #[error("Outdated version")]
    BadVersion,
    /// An error occurred in the client.
    #[error(transparent)]
    Client(#[from] sp_blockchain::Error),
}

/// A struct responsible for verifying block header.
pub struct HeaderVerifier<Block, Client> {
    client: Arc<Client>,
    chain_params: ChainParams,
    _phantom: PhantomData<Block>,
}

impl<Block, Client> Clone for HeaderVerifier<Block, Client> {
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
            chain_params: self.chain_params.clone(),
            _phantom: self._phantom,
        }
    }
}

impl<Block, Client> HeaderVerifier<Block, Client> {
    /// Constructs a new instance of [`HeaderVerifier`].
    pub fn new(client: Arc<Client>, chain_params: ChainParams) -> Self {
        Self {
            client,
            chain_params,
            _phantom: Default::default(),
        }
    }
}

impl<Block, Client> HeaderVerifier<Block, Client>
where
    Block: BlockT,
    Client: HeaderBackend<Block> + AuxStore,
{
    /// Validates the header and returns the block time, which is used for verifying the finality of
    /// transactions.
    ///
    /// The validation process includes:
    /// - Checking the proof of work.
    /// - Validating the block's timestamp:
    ///     - The time must not be more than 2 hours in the future.
    ///     - The time must be greater than the median time of the last 11 blocks.
    ///
    /// <https://github.com/bitcoin/bitcoin/blob/6f9db1ebcab4064065ccd787161bf2b87e03cc1f/src/validation.cpp#L4146>
    pub fn verify(&self, header: &BitcoinHeader) -> Result<u32, Error> {
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
            &self.chain_params.params,
            &self.client,
        );
        let expected_bits = expected_target.to_compact_lossy().to_consensus();

        let actual_target = header.target();

        if actual_target.to_compact_lossy().to_consensus() != expected_bits {
            return Err(Error::BadDifficultyBits {
                got: actual_target,
                expected: expected_target,
            });
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

        let version = header.version.to_consensus();

        if version < 2 && block_number >= self.chain_params.params.bip34_height
            || version < 3 && block_number >= self.chain_params.params.bip66_height
            || version < 4 && block_number >= self.chain_params.params.bip65_height
        {
            return Err(Error::BadVersion);
        }

        // BIP 113
        let lock_time_cutoff = if block_number >= self.chain_params.csv_height {
            let mtp = self.calculate_median_time_past(header);
            if header.time <= mtp {
                return Err(Error::TimeTooOld);
            }
            mtp
        } else {
            header.time
        };

        Ok(lock_time_cutoff)
    }

    /// Check if the proof-of-work is valid.
    pub fn has_valid_proof_of_work(&self, header: &BitcoinHeader) -> bool {
        let target = header.target();

        if target == Target::ZERO
            || target > Target::MAX
            || target > self.chain_params.params.max_attainable_target
        {
            return false;
        }

        header.validate_pow(target).is_ok()
    }

    /// Calculates the median time of the previous few blocks prior to the header (inclusive).
    fn calculate_median_time_past(&self, header: &BitcoinHeader) -> u32 {
        let mut timestamps = Vec::with_capacity(MEDIAN_TIME_SPAN);

        timestamps.push(header.time);

        let zero_hash = BlockHash::all_zeros();

        let mut block_hash = header.prev_blockhash;

        for _ in 0..MEDIAN_TIME_SPAN - 1 {
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
/// <https://github.com/bitcoin/bitcoin/blob/89b910711c004c21b7d67baa888073742f7f94f0/src/pow.cpp#L13>
fn get_next_work_required<Block, Client>(
    last_block_height: u32,
    last_block: BitcoinHeader,
    params: &Params,
    client: &Arc<Client>,
) -> Target
where
    Block: BlockT,
    Client: HeaderBackend<Block> + AuxStore,
{
    if params.no_pow_retargeting {
        return last_block.target();
    }

    let height = last_block_height + 1;

    let difficulty_adjustment_interval = params.difficulty_adjustment_interval() as u32;

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
            last_block.target().0,
            first_block_time.into(),
            last_block_time.into(),
            params,
        )
    } else {
        last_block.target()
    }
}

// <https://github.com/bitcoin/bitcoin/blob/89b910711c004c21b7d67baa888073742f7f94f0/src/pow.cpp#L49-L72>
fn calculate_next_work_required(
    previous_target: U256,
    first_block_time: u64,
    last_block_time: u64,
    params: &Params,
) -> Target {
    let mut actual_timespan = last_block_time.saturating_sub(first_block_time);

    let pow_target_timespan = params.pow_target_timespan;

    // Limit adjustment step.
    //
    // Note: The new difficulty is in [Difficulty_old * 1/4, Difficulty_old * 4].
    if actual_timespan < pow_target_timespan / 4 {
        actual_timespan = pow_target_timespan / 4;
    }

    if actual_timespan > pow_target_timespan * 4 {
        actual_timespan = pow_target_timespan * 4;
    }

    let pow_limit = params.max_attainable_target;

    // Retarget.
    let target = previous_target * actual_timespan.into();
    let target = Target(target / pow_target_timespan.into());

    if target > pow_limit {
        pow_limit
    } else {
        target
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::consensus::encode::deserialize_hex;

    #[test]
    fn test_calculate_next_work_required() {
        // block_354816
        let block_354816: BitcoinHeader = deserialize_hex
            ("020000003f99814a36d2a2043b1d4bf61a410f71828eca1decbf56000000000000000000b3762ed278ac44bb953e24262cfeb952d0abe6d3b7f8b74fd24e009b96b6cb965d674655dd1317186436e79d").unwrap();

        let expected_target = block_354816.target();

        // block_352800, first block in this period.
        let first_block: BitcoinHeader = deserialize_hex("0200000074c51c1cc53aaf478c643bb612da6bd17b268cd9bdccc4000000000000000000ccc0a2618a1f973dfac37827435b463abd18cbfd0f280a90432d3d78497a36cc02f33355f0171718b72a1dc7").unwrap();

        // block_354815, last block in this period.
        let last_block: BitcoinHeader = deserialize_hex("030000004c9c1b59250f30b8d360886a5433501120b056a000bdc0160000000000000000caca1bf0c55a5ba2299f9e60d10c01c679bb266c7df815ff776a1b97fd3a199ac1644655f01717182707bd59").unwrap();

        let new_target = calculate_next_work_required(
            last_block.target().0,
            first_block.time as u64,
            last_block.time as u64,
            &Params::new(bitcoin::Network::Bitcoin),
        );

        assert_eq!(
            new_target.to_compact_lossy().to_consensus(),
            expected_target.to_compact_lossy().to_consensus(),
            "Difficulty bits must match"
        );
    }
}
