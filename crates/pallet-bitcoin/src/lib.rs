//! # Bitcoin Pallet
//!
//! This pallet is minimalist, serving only to wrap Bitcoin transactions as Substrate extrinsics.
//! There is no UTXO state management - UTXOs are stored in native RocksDB storage outside the
//! runtime for O(1) lookups. This approach enables fast syncing without Merkle Patricia Trie
//! overhead.

// Ensure we're `no_std` when compiling for Wasm.
#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;
pub mod types;

use self::types::Transaction;
use frame_support::dispatch::DispatchResult;
use frame_support::weights::Weight;

// Re-export pallet items so that they can be accessed from the crate namespace.
pub use pallet::*;

/// Weight functions needed for `pallet_bitcoin`.
pub trait WeightInfo {
    /// Calculates the weight of [`Call::transact`].
    fn transact(btc_tx: &Transaction) -> Weight;
}

impl WeightInfo for () {
    fn transact(_: &Transaction) -> Weight {
        Weight::zero()
    }
}

/// A struct that implements the [`WeightInfo`] trait for Bitcoin transactions.
pub struct BitcoinTransactionWeight;

impl WeightInfo for BitcoinTransactionWeight {
    fn transact(btc_tx: &Transaction) -> Weight {
        let btc_weight = bitcoin::Transaction::from(btc_tx.clone()).weight().to_wu();
        Weight::from_parts(btc_weight, 0u64)
    }
}

#[frame_support::pallet]
pub mod pallet {
    use super::*;
    use frame_support::pallet_prelude::*;
    use frame_system::pallet_prelude::*;

    #[pallet::config]
    pub trait Config: frame_system::Config {
        type WeightInfo: WeightInfo;
    }

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::call(weight(<T as Config>::WeightInfo))]
    impl<T: Config> Pallet<T> {
        /// An unsigned extrinsic for embedding a Bitcoin transaction into the Substrate block.
        ///
        /// This only wraps the transaction - UTXO state is managed externally in native storage.
        #[pallet::call_index(0)]
        #[pallet::weight((T::WeightInfo::transact(btc_tx), DispatchClass::Normal, Pays::No))]
        pub fn transact(origin: OriginFor<T>, btc_tx: Transaction) -> DispatchResult {
            ensure_none(origin)?;
            // No-op: UTXO state is managed in native storage outside the runtime
            let _ = btc_tx;
            Ok(())
        }
    }

    #[pallet::event]
    pub enum Event<T: Config> {}
}
