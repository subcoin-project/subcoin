#![cfg_attr(not(feature = "std"), no_std)]
// TODO: This originates from `sp_api::decl_runtime_apis`.
#![allow(clippy::multiple_bound_locations)]

extern crate alloc;

use alloc::vec::Vec;
use codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_runtime::ConsensusEngineId;

/// The `ConsensusEngineId` of Bitcoin block hash.
pub const NAKAMOTO_HASH_ENGINE_ID: ConsensusEngineId = *b"hash";

/// The `ConsensusEngineId` of Bitcoin block header.
pub const NAKAMOTO_HEADER_ENGINE_ID: ConsensusEngineId = *b"hedr";

/// 1 BTC in satoshis
const COIN: u64 = 100_000_000;

/// Initial block reward
const INITIAL_SUBSIDY: u64 = 50 * COIN;

const HALVING_INTERVAL: u32 = 210_000;

const MAX_SCRIPT_SIZE: usize = 10_000;

/// Unspent transaction output.
#[derive(Debug, TypeInfo, Encode, Decode)]
pub struct Coin {
    /// Whether the coin is from a coinbase transaction.
    pub is_coinbase: bool,
    /// Transfer value in satoshis.
    pub amount: u64,
    // Block height at which this containing transaction was included.
    pub height: u32,
    /// Spending condition of the output.
    /// TODO: store the full script_pubkey offchain?
    pub script_pubkey: Vec<u8>,
}

impl MaxEncodedLen for Coin {
    fn max_encoded_len() -> usize {
        bool::max_encoded_len() + u64::max_encoded_len() + MAX_SCRIPT_SIZE
    }
}

/// Returns the amount of subsidy in satoshis at given height.
pub fn bitcoin_block_subsidy(height: u32) -> u64 {
    block_subsidy(height, HALVING_INTERVAL)
}

/// Returns the block subsidy at given height and halving interval.
fn block_subsidy(height: u32, subsidy_halving_interval: u32) -> u64 {
    let halvings = height / subsidy_halving_interval;
    // Force block reward to zero when right shift is undefined.
    if halvings >= 64 {
        return 0;
    }

    let mut subsidy = INITIAL_SUBSIDY;

    // Subsidy is cut in half every 210,000 blocks which will occur
    // approximately every 4 years.
    subsidy >>= halvings;

    subsidy
}

sp_api::decl_runtime_apis! {
    /// Subcoin API.
    pub trait Subcoin {
        /// Same as the original `execute_block()` with the removal
        /// of `state_root` check in the `final_checks()`.
        fn execute_block_without_state_root_check(block: Block);

        /// Finalize block without checking the extrinsics_root and state_root.
        fn finalize_block_without_checks(header: Block::Header);
    }
}
