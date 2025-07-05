//! # Bitcoin Pallet
//!
//! This pallet is designed to be minimalist, containing only one storage item for maintaining
//! the state of the UTXO (Unspent Transaction Output) set by processing the inputs and outputs
//! of each Bitcoin transaction wrapped in [`Call::transact`]. There is no verification logic
//! within the pallet, all validation work should be performed outside the runtime. This approach
//! simplifies off-runtime execution, allowing for easier syncing performance optimization off
//! chain.

// Ensure we're `no_std` when compiling for Wasm.
#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;
pub mod types;

use self::types::{OutPoint, Transaction, Txid};
use frame_support::dispatch::DispatchResult;
use frame_support::weights::Weight;
use sp_runtime::SaturatedConversion;
use sp_runtime::traits::BlockNumberProvider;
use sp_std::prelude::*;
use sp_std::vec::Vec;
use subcoin_runtime_primitives::Coin;

// Re-export pallet items so that they can be accessed from the crate namespace.
pub use pallet::*;

/// Transaction output index.
pub type OutputIndex = u32;

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
        #[pallet::call_index(0)]
        #[pallet::weight((T::WeightInfo::transact(btc_tx), DispatchClass::Normal, Pays::No))]
        pub fn transact(origin: OriginFor<T>, btc_tx: Transaction) -> DispatchResult {
            ensure_none(origin)?;

            Self::process_bitcoin_transaction(btc_tx.into());

            Ok(())
        }
    }

    #[pallet::event]
    pub enum Event<T: Config> {}

    /// UTXO set.
    ///
    /// (Txid, OutputIndex(vout), Coin)
    ///
    /// Note: There is a special case in Bitcoin that the outputs in the genesis block are excluded from the UTXO set.
    #[pallet::storage]
    pub type Coins<T> =
        StorageDoubleMap<_, Identity, Txid, Identity, OutputIndex, Coin, OptionQuery>;

    /// Size of the entire UTXO set.
    #[pallet::storage]
    pub type CoinsCount<T> = StorageValue<_, u64, ValueQuery>;
}

/// Returns the storage key for the referenced output.
pub fn coin_storage_key<T: Config>(bitcoin_txid: bitcoin::Txid, vout: OutputIndex) -> Vec<u8> {
    use frame_support::storage::generator::StorageDoubleMap;

    let txid = Txid::from_bitcoin_txid(bitcoin_txid);
    Coins::<T>::storage_double_map_final_key(txid, vout)
}

/// Returns the final storage prefix for the storage item `Coins`.
pub fn coin_storage_prefix<T: Config>() -> [u8; 32] {
    use frame_support::StoragePrefixedMap;

    Coins::<T>::final_prefix()
}

impl<T: Config> Pallet<T> {
    pub fn coins_count() -> u64 {
        CoinsCount::<T>::get()
    }

    fn process_bitcoin_transaction(tx: bitcoin::Transaction) {
        let txid = tx.compute_txid();
        let is_coinbase = tx.is_coinbase();
        let height = frame_system::Pallet::<T>::current_block_number();

        // Check if transaction falls under the BIP30 exception heights
        const BIP30_EXCEPTION_HEIGHTS: &[u32] = &[91722, 91812];
        if is_coinbase && BIP30_EXCEPTION_HEIGHTS.contains(&height.saturated_into()) {
            // Skip adding the outputs to UTXO set if it is a BIP30 exception.
            return;
        }

        // Collect all new coins to be added to the UTXO set, skipping OP_RETURN outputs.
        let new_coins: Vec<_> = tx
            .output
            .into_iter()
            .enumerate()
            .filter_map(|(index, txout)| {
                let out_point = bitcoin::OutPoint {
                    txid,
                    vout: index as u32,
                };

                // OP_RETURN outputs are not added to the UTXO set.
                //
                // TODO: handle OP_RETURN data properly as they are super valuable.
                if txout.script_pubkey.is_op_return() {
                    None
                } else {
                    let coin = Coin {
                        is_coinbase,
                        amount: txout.value.to_sat(),
                        script_pubkey: txout.script_pubkey.into_bytes(),
                        height: height.saturated_into(),
                    };
                    Some((out_point, coin))
                }
            })
            .collect();

        let num_created = new_coins.len();

        if is_coinbase {
            // Insert new UTXOs for coinbase transaction.
            for (out_point, coin) in new_coins {
                let OutPoint { txid, output_index } = OutPoint::from(out_point);
                Coins::<T>::insert(txid, output_index, coin);
            }
            CoinsCount::<T>::mutate(|v| {
                *v += num_created as u64;
            });
            return;
        }

        let num_consumed = tx.input.len();

        // Process the inputs to remove consumed UTXOs.
        for input in tx.input {
            let previous_output = input.previous_output;
            let OutPoint { txid, output_index } = OutPoint::from(previous_output);
            if let Some(_spent) = Coins::<T>::take(txid, output_index) {
            } else {
                panic!("Corruputed state, UTXO {previous_output:?} not found");
            }
        }

        // Insert new UTXOs for non-coinbase transaction.
        for (out_point, coin) in new_coins {
            let OutPoint { txid, output_index } = OutPoint::from(out_point);
            Coins::<T>::insert(txid, output_index, coin);
        }

        CoinsCount::<T>::mutate(|v| {
            *v = *v + num_created as u64 - num_consumed as u64;
        });
    }
}
