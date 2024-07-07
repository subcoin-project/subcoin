// This file is part of Substrate.

// Copyright (C) Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Subcoin runtime is a minimalistic Substrate runtime, it does not implement all the typical
//! runtime APIs the other runtimes would do.

#![cfg_attr(not(feature = "std"), no_std)]

// Make the WASM binary available.
#[cfg(feature = "std")]
include!(concat!(env!("OUT_DIR"), "/wasm_binary.rs"));

use frame_support::genesis_builder_helper::{build_state, get_preset};
use frame_support::{construct_runtime, derive_impl, parameter_types};
use frame_system::pallet_prelude::*;
use pallet_executive::Executive;
use sp_api::impl_runtime_apis;
use sp_core::{ConstU32, OpaqueMetadata};
use sp_inherents::{CheckInherentsResult, InherentData};
use sp_runtime::transaction_validity::{TransactionSource, TransactionValidity};
use sp_runtime::{ApplyExtrinsicResult, ExtrinsicInclusionMode};
use sp_std::boxed::Box;
use sp_std::vec;
use sp_std::vec::Vec;
#[cfg(feature = "std")]
use sp_version::NativeVersion;
use sp_version::{create_runtime_str, runtime_version, RuntimeVersion};

#[runtime_version]
pub const VERSION: RuntimeVersion = RuntimeVersion {
    spec_name: create_runtime_str!("subcoin"),
    impl_name: create_runtime_str!("subcoin"),
    authoring_version: 0,
    spec_version: 0,
    impl_version: 0,
    apis: RUNTIME_API_VERSIONS,
    transaction_version: 0,
    state_version: 0,
};

/// The version information used to identify this runtime when compiled natively.
#[cfg(feature = "std")]
pub fn native_version() -> NativeVersion {
    NativeVersion {
        runtime_version: VERSION,
        can_author_with: Default::default(),
    }
}

type SignedExtra = (
    frame_system::CheckNonZeroSender<Runtime>,
    frame_system::CheckSpecVersion<Runtime>,
    frame_system::CheckGenesis<Runtime>,
);

parameter_types! {
    pub const Version: RuntimeVersion = VERSION;
}

construct_runtime!(
    pub struct Runtime {
        System: frame_system,
        Bitcoin: pallet_bitcoin,
    }
);

#[derive_impl(frame_system::config_preludes::SolochainDefaultConfig as frame_system::DefaultConfig)]
impl frame_system::Config for Runtime {
    type Block = Block;
    type Version = Version;
    type BlockHashCount = ConstU32<1024>;
}

impl pallet_bitcoin::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = ();
}

type Signature = crate::types_common::Signature;
type Block = crate::types_common::BlockOf<Runtime, SignedExtra>;
// TODO: Proper address
pub type Address = sp_runtime::MultiAddress<interface::AccountId, ()>;
pub type Header = HeaderFor<Runtime>;
pub type UncheckedExtrinsic =
    sp_runtime::generic::UncheckedExtrinsic<Address, RuntimeCall, Signature, SignedExtra>;

type RuntimeExecutive =
    Executive<Runtime, Block, frame_system::ChainContext<Runtime>, Runtime, AllPalletsWithSystem>;

impl_runtime_apis! {
    impl sp_api::Core<Block> for Runtime {
        fn version() -> RuntimeVersion {
            VERSION
        }

        fn execute_block(block: Block) {
            RuntimeExecutive::execute_block(block)
        }

        fn initialize_block(header: &Header) -> ExtrinsicInclusionMode {
            RuntimeExecutive::initialize_block(header)
        }
    }

    impl sp_api::Metadata<Block> for Runtime {
        fn metadata() -> OpaqueMetadata {
            OpaqueMetadata::new(Runtime::metadata().into())
        }

        fn metadata_at_version(version: u32) -> Option<OpaqueMetadata> {
            Runtime::metadata_at_version(version)
        }

        fn metadata_versions() -> Vec<u32> {
            Runtime::metadata_versions()
        }
    }

    // Cannot be removed as required by frame-benchmarking-cli.
    impl sp_block_builder::BlockBuilder<Block> for Runtime {
        fn apply_extrinsic(extrinsic: ExtrinsicFor<Runtime>) -> ApplyExtrinsicResult {
            RuntimeExecutive::apply_extrinsic(extrinsic)
        }

        fn finalize_block() -> HeaderFor<Runtime> {
            unimplemented!("BlockBuilder::finalize_block() is useless in Subcoin")
        }

        fn inherent_extrinsics(data: InherentData) -> Vec<ExtrinsicFor<Runtime>> {
            data.create_extrinsics()
        }

        fn check_inherents(
            block: Block,
            data: InherentData,
        ) -> CheckInherentsResult {
            data.check_extrinsics(&block)
        }
    }

    impl sp_transaction_pool::runtime_api::TaggedTransactionQueue<Block> for Runtime {
        fn validate_transaction(
            source: TransactionSource,
            tx: ExtrinsicFor<Runtime>,
            block_hash: <Runtime as frame_system::Config>::Hash,
        ) -> TransactionValidity {
            RuntimeExecutive::validate_transaction(source, tx, block_hash)
        }
    }

    // Cannot be removed as required by `sc_service::spawn_tasks()`.
    //
    // TODO: remove if we introduce our own version of spawn_tasks.
    impl sp_session::SessionKeys<Block> for Runtime {
        fn generate_session_keys(_seed: Option<Vec<u8>>) -> Vec<u8> {
            Default::default()
        }

        fn decode_session_keys(
            _encoded: Vec<u8>,
        ) -> Option<Vec<(Vec<u8>, sp_session::KeyTypeId)>> {
            Default::default()
        }
    }

    // Cannot be removed as required by SystemApiServer in rpc.
    impl frame_system_rpc_runtime_api::AccountNonceApi<Block, interface::AccountId, interface::Nonce> for Runtime {
        fn account_nonce(account: interface::AccountId) -> interface::Nonce {
            System::account_nonce(account)
        }
    }

    impl sp_genesis_builder::GenesisBuilder<Block> for Runtime {
        fn build_state(config: Vec<u8>) -> sp_genesis_builder::Result {
            build_state::<RuntimeGenesisConfig>(config)
        }

        fn get_preset(id: &Option<sp_genesis_builder::PresetId>) -> Option<Vec<u8>> {
            get_preset::<RuntimeGenesisConfig>(id, |_| None)
        }

        fn preset_names() -> Vec<sp_genesis_builder::PresetId> {
            vec![]
        }
    }

    impl subcoin_runtime_primitives::Subcoin<Block> for Runtime {
        fn execute_block_without_state_root_check(block: Block) {
            RuntimeExecutive::execute_block_without_state_root_check(block)
        }

        fn finalize_block_without_checks(header: HeaderFor<Runtime>) {
            RuntimeExecutive::finalize_block_without_checks(header);
        }
    }
}

/// A set of opinionated types aliases commonly used in runtimes.
///
/// This is one set of opinionated types. They are compatible with one another, but are not
/// guaranteed to work if you start tweaking a portion.
///
/// Some note-worthy opinions in this prelude:
///
/// - `u32` block number.
/// - [`sp_runtime::MultiAddress`] and [`sp_runtime::MultiSignature`] are used as the account id
///   and signature types. This implies that this prelude can possibly used with an
///   "account-index" system (eg `pallet-indices`). And, in any case, it should be paired with
///   `AccountIdLookup` in [`frame_system::Config::Lookup`].
mod types_common {
    use frame_system::Config as SysConfig;
    use sp_runtime::{generic, traits, OpaqueExtrinsic};

    /// A signature type compatible capably of handling multiple crypto-schemes.
    pub type Signature = sp_runtime::MultiSignature;

    /// The corresponding account-id type of [`Signature`].
    pub type AccountId =
        <<Signature as traits::Verify>::Signer as traits::IdentifyAccount>::AccountId;

    /// The block-number type, which should be fed into [`frame_system::Config`].
    pub type BlockNumber = u32;

    /// TODO: Ideally we want the hashing type to be equal to SysConfig::Hashing?
    type HeaderInner = generic::Header<BlockNumber, traits::BlakeTwo256>;

    // NOTE: `AccountIndex` is provided for future compatibility, if you want to introduce
    // something like `pallet-indices`.
    type ExtrinsicInner<T, Extra, AccountIndex = ()> = generic::UncheckedExtrinsic<
        sp_runtime::MultiAddress<AccountId, AccountIndex>,
        <T as SysConfig>::RuntimeCall,
        Signature,
        Extra,
    >;

    /// The block type, which should be fed into [`frame_system::Config`].
    ///
    /// Should be parameterized with `T: frame_system::Config` and a tuple of `SignedExtension`.
    /// When in doubt, use [`SystemSignedExtensionsOf`].
    // Note that this cannot be dependent on `T` for block-number because it would lead to a
    // circular dependency (self-referential generics).
    pub type BlockOf<T, Extra = ()> = generic::Block<HeaderInner, ExtrinsicInner<T, Extra>>;

    /// The opaque block type. This is the same [`BlockOf`], but it has
    /// [`sp_runtime::OpaqueExtrinsic`] as its final extrinsic type.
    ///
    /// This should be provided to the client side as the extrinsic type.
    pub type OpaqueBlock = generic::Block<HeaderInner, OpaqueExtrinsic>;
}

/// Some re-exports that the node side code needs to know. Some are useful in this context as well.
///
/// Other types should preferably be private.
// TODO: this should be standardized in some way, see:
// https://github.com/paritytech/substrate/issues/10579#issuecomment-1600537558
pub mod interface {
    use super::Runtime;

    pub type Block = super::Block;
    pub use crate::types_common::OpaqueBlock;
    pub type AccountId = <Runtime as frame_system::Config>::AccountId;
    pub type Nonce = <Runtime as frame_system::Config>::Nonce;
    pub type Hash = <Runtime as frame_system::Config>::Hash;
    pub type Balance = u128;
}
