#![cfg_attr(not(feature = "std"), no_std)]
// TODO: This originates from `sp_api::decl_runtime_apis`.
#![allow(clippy::multiple_bound_locations)]

extern crate alloc;

use alloc::vec::Vec;
use bitcoin::consensus::Encodable;
use codec::{Decode, DecodeWithMemTracking, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_core::H256;
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
#[derive(Debug, Clone, PartialEq, Eq, TypeInfo, Encode, Decode)]
#[cfg_attr(feature = "std", derive(serde::Serialize, serde::Deserialize))]
pub struct Coin {
    /// Whether the coin is from a coinbase transaction.
    pub is_coinbase: bool,
    /// Transfer value in satoshis.
    pub amount: u64,
    // Block height at which this containing transaction was included.
    pub height: u32,
    /// Spending condition of the output.
    pub script_pubkey: Vec<u8>,
}

impl MaxEncodedLen for Coin {
    fn max_encoded_len() -> usize {
        bool::max_encoded_len() + u64::max_encoded_len() + MAX_SCRIPT_SIZE
    }
}

/// Wrapper type for representing [`bitcoin::Txid`] in runtime, stored in reversed byte order.
#[derive(
    Debug,
    Clone,
    Copy,
    TypeInfo,
    Encode,
    Decode,
    DecodeWithMemTracking,
    MaxEncodedLen,
    PartialEq,
    Eq,
)]
pub struct Txid(H256);

impl From<bitcoin::Txid> for Txid {
    fn from(txid: bitcoin::Txid) -> Self {
        let mut bytes = Vec::with_capacity(32);
        txid.consensus_encode(&mut bytes)
            .expect("txid must be encoded correctly; qed");

        bytes.reverse();

        let bytes: [u8; 32] = bytes
            .try_into()
            .expect("Bitcoin txid is sha256 hash which must fit into [u8; 32]; qed");

        Self(H256::from(bytes))
    }
}

impl From<Txid> for bitcoin::Txid {
    fn from(txid: Txid) -> Self {
        let mut bytes = txid.encode();
        bytes.reverse();
        bitcoin::consensus::Decodable::consensus_decode(&mut bytes.as_slice())
            .expect("Decode must succeed as txid was guaranteed to be encoded correctly; qed")
    }
}

/// A reference to a transaction output.
#[derive(
    Debug, Clone, Encode, Decode, DecodeWithMemTracking, TypeInfo, PartialEq, Eq, MaxEncodedLen,
)]
pub struct OutPoint {
    /// The transaction ID of the referenced output.
    pub txid: Txid,
    /// The index of the output within the referenced transaction.
    pub vout: u32,
}

impl From<bitcoin::OutPoint> for OutPoint {
    fn from(out_point: bitcoin::OutPoint) -> Self {
        Self {
            txid: out_point.txid.into(),
            vout: out_point.vout,
        }
    }
}

impl From<OutPoint> for bitcoin::OutPoint {
    fn from(val: OutPoint) -> Self {
        bitcoin::OutPoint {
            txid: val.txid.into(),
            vout: val.vout,
        }
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
    pub trait SubcoinApi {
        /// Same as the original `execute_block()` with the removal
        /// of `state_root` check in the `final_checks()`.
        fn execute_block_without_state_root_check(block: Block::LazyBlock);

        /// Finalize block without checking the extrinsics_root and state_root.
        fn finalize_block_without_checks(header: Block::Header);

        /// Returns the number of total coins (i.e., the size of UTXO set).
        fn coins_count() -> u64;

        /// Query UTXOs by outpoints.
        ///
        /// Returns a vector of the same length as the input, with `Some(Coin)` for
        /// UTXOs that exist and `None` for those that don't.
        fn get_utxos(outpoints: Vec<OutPoint>) -> Vec<Option<Coin>>;
    }
}
