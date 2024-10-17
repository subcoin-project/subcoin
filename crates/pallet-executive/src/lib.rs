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

#![cfg_attr(not(feature = "std"), no_std)]

//! # Executive Module
//!
//! This is a fork of the upstream `frame-executive`. Most of the superior features have been
//! removed, this pallet aims to provide the minimum stable interfaces for runtime.
//!
//! The Executive module acts as the orchestration layer for the runtime. It dispatches incoming
//! extrinsic calls to the respective modules in the runtime.
//!
//! ## Overview
//!
//! The executive module is not a typical pallet providing functionality around a specific feature.
//! It is a cross-cutting framework component for the FRAME. It works in conjunction with the
//! [FRAME System module](../frame_system/index.html) to perform these cross-cutting functions.
//!
//! The Executive module provides functions to:
//!
//! - Check transaction validity.
//! - Initialize a block.
//! - Apply extrinsics.
//! - Execute a block.
//! - Finalize a block.
//! - Start an off-chain worker.
//!
//! The flow of their application in a block is explained in the [block flowchart](block_flowchart).
//!
//! ### Implementations
//!
//! The Executive module provides the following implementations:
//!
//! - `ExecuteBlock`: Trait that can be used to execute a block.
//! - `Executive`: Type that can be used to make the FRAME available from the runtime.

#[cfg(doc)]
#[cfg_attr(doc, aquamarine::aquamarine)]
/// # Block Execution
///
/// These are the steps of block execution as done by [`Executive::execute_block`]. A block is
/// invalid if any of them fail.
///
/// ```mermaid
/// flowchart TD
///     Executive::execute_block --> on_runtime_upgrade
///     on_runtime_upgrade --> System::initialize
///     Executive::initialize_block --> System::initialize
///     System::initialize --> on_initialize
///     on_initialize --> PreInherents[System::PreInherents]
///     PreInherents --> Inherents[Apply Inherents]
///     Inherents --> PostInherents[System::PostInherents]
///     PostInherents --> Check{MBM ongoing?}
///     Check -->|No| poll
///     Check -->|Yes| post_transactions_2[System::PostTransaction]
///     post_transactions_2 --> Step[MBMs::step]
///     Step --> on_finalize
///     poll --> transactions[Apply Transactions]
///     transactions --> post_transactions_1[System::PostTransaction]
///     post_transactions_1 --> CheckIdle{Weight remaining?}
///     CheckIdle -->|Yes| on_idle
///     CheckIdle -->|No| on_finalize
///     on_idle --> on_finalize
/// ```
pub mod block_flowchart {}

// #[cfg(test)]
// mod tests;

use codec::{Codec, Encode};
use frame_support::defensive_assert;
use frame_support::dispatch::{DispatchClass, DispatchInfo, GetDispatchInfo, PostDispatchInfo};
use frame_support::migrations::MultiStepMigrator;
use frame_support::pallet_prelude::InvalidTransaction;
use frame_support::traits::{
    BeforeAllRuntimeMigrations, EnsureInherentsAreFirst, ExecuteBlock, OffchainWorker, OnFinalize,
    OnIdle, OnInitialize, OnPoll, OnRuntimeUpgrade, PostInherents, PostTransactions,
};
use frame_support::weights::{Weight, WeightMeter};
use frame_system::pallet_prelude::BlockNumberFor;
use sp_runtime::generic::Digest;
use sp_runtime::traits::{
    self, Applyable, CheckEqual, Checkable, Dispatchable, Header, NumberFor, One, ValidateUnsigned,
    Zero,
};
use sp_runtime::transaction_validity::{TransactionSource, TransactionValidity};
use sp_runtime::{ApplyExtrinsicResult, ExtrinsicInclusionMode};
use sp_std::marker::PhantomData;
use sp_std::prelude::*;

#[cfg(feature = "try-runtime")]
use ::{
    frame_support::{
        traits::{TryDecodeEntireStorage, TryDecodeEntireStorageError, TryState},
        StorageNoopGuard,
    },
    frame_try_runtime::{TryStateSelect, UpgradeCheckSelect},
    log,
    sp_runtime::TryRuntimeError,
};

#[allow(dead_code)]
const LOG_TARGET: &str = "runtime::executive";

pub type CheckedOf<E, C> = <E as Checkable<C>>::Checked;
pub type CallOf<E, C> = <CheckedOf<E, C> as Applyable>::Call;
pub type OriginOf<E, C> = <CallOf<E, C> as Dispatchable>::RuntimeOrigin;

/// Main entry point for certain runtime actions as e.g. `execute_block`.
///
/// Generic parameters:
/// - `System`: Something that implements `frame_system::Config`
/// - `Block`: The block type of the runtime
/// - `Context`: The context that is used when checking an extrinsic.
/// - `UnsignedValidator`: The unsigned transaction validator of the runtime.
/// - `AllPalletsWithSystem`: Tuple that contains all pallets including frame system pallet. Will be
///   used to call hooks e.g. `on_initialize`.
/// - `OnRuntimeUpgrade`: Custom logic that should be called after a runtime upgrade. Modules are
///   already called by `AllPalletsWithSystem`. It will be called before all modules will be called.
pub struct Executive<
    System,
    Block,
    Context,
    UnsignedValidator,
    AllPalletsWithSystem,
    OnRuntimeUpgrade = (),
>(
    PhantomData<(
        System,
        Block,
        Context,
        UnsignedValidator,
        AllPalletsWithSystem,
        OnRuntimeUpgrade,
    )>,
);

impl<
        System: frame_system::Config + EnsureInherentsAreFirst<Block>,
        Block: traits::Block<
            Header = frame_system::pallet_prelude::HeaderFor<System>,
            Hash = System::Hash,
        >,
        Context: Default,
        UnsignedValidator,
        AllPalletsWithSystem: OnRuntimeUpgrade
            + BeforeAllRuntimeMigrations
            + OnInitialize<BlockNumberFor<System>>
            + OnIdle<BlockNumberFor<System>>
            + OnFinalize<BlockNumberFor<System>>
            + OffchainWorker<BlockNumberFor<System>>
            + OnPoll<BlockNumberFor<System>>,
        COnRuntimeUpgrade: OnRuntimeUpgrade,
    > ExecuteBlock<Block>
    for Executive<
        System,
        Block,
        Context,
        UnsignedValidator,
        AllPalletsWithSystem,
        COnRuntimeUpgrade,
    >
where
    Block::Extrinsic: Checkable<Context> + Codec,
    CheckedOf<Block::Extrinsic, Context>: Applyable + GetDispatchInfo,
    CallOf<Block::Extrinsic, Context>:
        Dispatchable<Info = DispatchInfo, PostInfo = PostDispatchInfo>,
    OriginOf<Block::Extrinsic, Context>: From<Option<System::AccountId>>,
    UnsignedValidator: ValidateUnsigned<Call = CallOf<Block::Extrinsic, Context>>,
{
    fn execute_block(block: Block) {
        Executive::<
            System,
            Block,
            Context,
            UnsignedValidator,
            AllPalletsWithSystem,
            COnRuntimeUpgrade,
        >::execute_block(block);
    }
}

#[cfg(feature = "try-runtime")]
impl<
        System: frame_system::Config + EnsureInherentsAreFirst<Block>,
        Block: traits::Block<
            Header = frame_system::pallet_prelude::HeaderFor<System>,
            Hash = System::Hash,
        >,
        Context: Default,
        UnsignedValidator,
        AllPalletsWithSystem: OnRuntimeUpgrade
            + BeforeAllRuntimeMigrations
            + OnInitialize<BlockNumberFor<System>>
            + OnIdle<BlockNumberFor<System>>
            + OnFinalize<BlockNumberFor<System>>
            + OffchainWorker<BlockNumberFor<System>>
            + OnPoll<BlockNumberFor<System>>
            + TryState<BlockNumberFor<System>>
            + TryDecodeEntireStorage,
        COnRuntimeUpgrade: OnRuntimeUpgrade,
    > Executive<System, Block, Context, UnsignedValidator, AllPalletsWithSystem, COnRuntimeUpgrade>
where
    Block::Extrinsic: Checkable<Context> + Codec,
    CheckedOf<Block::Extrinsic, Context>: Applyable + GetDispatchInfo,
    CallOf<Block::Extrinsic, Context>:
        Dispatchable<Info = DispatchInfo, PostInfo = PostDispatchInfo>,
    OriginOf<Block::Extrinsic, Context>: From<Option<System::AccountId>>,
    UnsignedValidator: ValidateUnsigned<Call = CallOf<Block::Extrinsic, Context>>,
{
    /// Execute given block, but don't as strict is the normal block execution.
    ///
    /// Some checks can be disabled via:
    ///
    /// - `state_root_check`
    /// - `signature_check`
    ///
    /// Should only be used for testing ONLY.
    pub fn try_execute_block(
        block: Block,
        state_root_check: bool,
        signature_check: bool,
        select: frame_try_runtime::TryStateSelect,
    ) -> Result<Weight, &'static str> {
        log::info!(
            target: LOG_TARGET,
            "try-runtime: executing block #{:?} / state root check: {:?} / signature check: {:?} / try-state-select: {:?}",
            block.header().number(),
            state_root_check,
            signature_check,
            select,
        );

        let mode = Self::initialize_block(block.header());
        let num_inherents = Self::initial_checks(&block) as usize;
        let (header, extrinsics) = block.deconstruct();

        // Check if there are any forbidden non-inherents in the block.
        if mode == ExtrinsicInclusionMode::OnlyInherents && extrinsics.len() > num_inherents {
            return Err("Only inherents allowed".into());
        }

        let try_apply_extrinsic = |uxt: Block::Extrinsic| -> ApplyExtrinsicResult {
            sp_io::init_tracing();
            let encoded = uxt.encode();
            let encoded_len = encoded.len();

            let is_inherent = System::is_inherent(&uxt);
            // skip signature verification.
            let xt = if signature_check {
                uxt.check(&Default::default())
            } else {
                uxt.unchecked_into_checked_i_know_what_i_am_doing(&Default::default())
            }?;

            let dispatch_info = xt.get_dispatch_info();
            if !is_inherent && !<frame_system::Pallet<System>>::inherents_applied() {
                Self::inherents_applied();
            }

            <frame_system::Pallet<System>>::note_extrinsic(encoded);
            let r = Applyable::apply::<UnsignedValidator>(xt, &dispatch_info, encoded_len)?;

            if r.is_err() && dispatch_info.class == DispatchClass::Mandatory {
                return Err(InvalidTransaction::BadMandatory.into());
            }

            <frame_system::Pallet<System>>::note_applied_extrinsic(&r, dispatch_info);

            Ok(r.map(|_| ()).map_err(|e| e.error))
        };

        // Apply extrinsics:
        for e in extrinsics.iter() {
            if let Err(err) = try_apply_extrinsic(e.clone()) {
                log::error!(
                    target: LOG_TARGET, "transaction {:?} failed due to {:?}. Aborting the rest of the block execution.",
                    e,
                    err,
                );
                break;
            }
        }

        // In this case there were no transactions to trigger this state transition:
        if !<frame_system::Pallet<System>>::inherents_applied() {
            Self::inherents_applied();
        }

        // post-extrinsics book-keeping
        <frame_system::Pallet<System>>::note_finished_extrinsics();
        <System as frame_system::Config>::PostTransactions::post_transactions();

        Self::on_idle_hook(*header.number());
        Self::on_finalize_hook(*header.number());

        // run the try-state checks of all pallets, ensuring they don't alter any state.
        let _guard = frame_support::StorageNoopGuard::default();
        <AllPalletsWithSystem as frame_support::traits::TryState<
			BlockNumberFor<System>,
		>>::try_state(*header.number(), select.clone())
		.map_err(|e| {
			log::error!(target: LOG_TARGET, "failure: {:?}", e);
			e
		})?;
        if select.any() {
            let res = AllPalletsWithSystem::try_decode_entire_state();
            Self::log_decode_result(res)?;
        }
        drop(_guard);

        // do some of the checks that would normally happen in `final_checks`, but perhaps skip
        // the state root check.
        {
            let new_header = <frame_system::Pallet<System>>::finalize();
            let items_zip = header
                .digest()
                .logs()
                .iter()
                .zip(new_header.digest().logs().iter());
            for (header_item, computed_item) in items_zip {
                header_item.check_equal(computed_item);
                assert!(
                    header_item == computed_item,
                    "Digest item must match that calculated."
                );
            }

            if state_root_check {
                let storage_root = new_header.state_root();
                header.state_root().check_equal(storage_root);
                assert!(
                    header.state_root() == storage_root,
                    "Storage root must match that calculated."
                );
            }

            assert!(
                header.extrinsics_root() == new_header.extrinsics_root(),
                "Transaction trie root must be valid.",
            );
        }

        log::info!(
            target: LOG_TARGET,
            "try-runtime: Block #{:?} successfully executed",
            header.number(),
        );

        Ok(frame_system::Pallet::<System>::block_weight().total())
    }

    /// Execute all Migrations of this runtime.
    ///
    /// The `checks` param determines whether to execute `pre/post_upgrade` and `try_state` hooks.
    ///
    /// [`frame_system::LastRuntimeUpgrade`] is set to the current runtime version after
    /// migrations execute. This is important for idempotency checks, because some migrations use
    /// this value to determine whether or not they should execute.
    pub fn try_runtime_upgrade(checks: UpgradeCheckSelect) -> Result<Weight, TryRuntimeError> {
        let before_all_weight =
            <AllPalletsWithSystem as BeforeAllRuntimeMigrations>::before_all_runtime_migrations();
        let try_on_runtime_upgrade_weight =
			<(COnRuntimeUpgrade, AllPalletsWithSystem) as OnRuntimeUpgrade>::try_on_runtime_upgrade(
				checks.pre_and_post(),
			)?;

        frame_system::LastRuntimeUpgrade::<System>::put(
            frame_system::LastRuntimeUpgradeInfo::from(
                <System::Version as frame_support::traits::Get<_>>::get(),
            ),
        );

        // Nothing should modify the state after the migrations ran:
        let _guard = StorageNoopGuard::default();

        // The state must be decodable:
        if checks.any() {
            let res = AllPalletsWithSystem::try_decode_entire_state();
            Self::log_decode_result(res)?;
        }

        // Check all storage invariants:
        if checks.try_state() {
            AllPalletsWithSystem::try_state(
                frame_system::Pallet::<System>::block_number(),
                TryStateSelect::All,
            )?;
        }

        Ok(before_all_weight.saturating_add(try_on_runtime_upgrade_weight))
    }

    /// Logs the result of trying to decode the entire state.
    fn log_decode_result(
        res: Result<usize, Vec<TryDecodeEntireStorageError>>,
    ) -> Result<(), TryRuntimeError> {
        match res {
            Ok(bytes) => {
                log::info!(
                    target: LOG_TARGET,
                    "✅ Entire runtime state decodes without error. {} bytes total.",
                    bytes
                );

                Ok(())
            }
            Err(errors) => {
                log::error!(
                    target: LOG_TARGET,
                    "`try_decode_entire_state` failed with {} errors",
                    errors.len(),
                );

                for (i, err) in errors.iter().enumerate() {
                    // We log the short version to `error` and then the full debug info to `debug`:
                    log::error!(target: LOG_TARGET, "- {i}. error: {err}");
                    log::debug!(target: LOG_TARGET, "- {i}. error: {err:?}");
                }

                Err("`try_decode_entire_state` failed".into())
            }
        }
    }
}

impl<
        System: frame_system::Config + EnsureInherentsAreFirst<Block>,
        Block: traits::Block<
            Header = frame_system::pallet_prelude::HeaderFor<System>,
            Hash = System::Hash,
        >,
        Context: Default,
        UnsignedValidator,
        AllPalletsWithSystem: OnRuntimeUpgrade
            + BeforeAllRuntimeMigrations
            + OnInitialize<BlockNumberFor<System>>
            + OnIdle<BlockNumberFor<System>>
            + OnFinalize<BlockNumberFor<System>>
            + OffchainWorker<BlockNumberFor<System>>
            + OnPoll<BlockNumberFor<System>>,
        COnRuntimeUpgrade: OnRuntimeUpgrade,
    > Executive<System, Block, Context, UnsignedValidator, AllPalletsWithSystem, COnRuntimeUpgrade>
where
    Block::Extrinsic: Checkable<Context> + Codec,
    CheckedOf<Block::Extrinsic, Context>: Applyable + GetDispatchInfo,
    CallOf<Block::Extrinsic, Context>:
        Dispatchable<Info = DispatchInfo, PostInfo = PostDispatchInfo>,
    OriginOf<Block::Extrinsic, Context>: From<Option<System::AccountId>>,
    UnsignedValidator: ValidateUnsigned<Call = CallOf<Block::Extrinsic, Context>>,
{
    /// Start the execution of a particular block.
    pub fn initialize_block(
        header: &frame_system::pallet_prelude::HeaderFor<System>,
    ) -> ExtrinsicInclusionMode {
        sp_io::init_tracing();
        sp_tracing::enter_span!(sp_tracing::Level::TRACE, "init_block");

        let digests = Self::extract_pre_digest(header);

        // Reset events before apply runtime upgrade hook.
        // This is required to preserve events from runtime upgrade hook.
        // This means the format of all the event related storages must always be compatible.
        <frame_system::Pallet<System>>::reset_events();

        <frame_system::Pallet<System>>::initialize(header.number(), header.parent_hash(), &digests);

        let weight = <System::BlockWeights as frame_support::traits::Get<_>>::get().base_block;
        <frame_system::Pallet<System>>::register_extra_weight_unchecked(
            weight,
            DispatchClass::Mandatory,
        );

        frame_system::Pallet::<System>::note_finished_initialize();

        ExtrinsicInclusionMode::AllExtrinsics
    }

    fn extract_pre_digest(header: &frame_system::pallet_prelude::HeaderFor<System>) -> Digest {
        let mut digest = <Digest>::default();
        header.digest().logs().iter().for_each(|d| {
            if d.as_pre_runtime().is_some() {
                digest.push(d.clone())
            }
        });
        digest
    }

    /// Returns the number of inherents in the block.
    fn initial_checks(block: &Block) -> u32 {
        sp_tracing::enter_span!(sp_tracing::Level::TRACE, "initial_checks");
        let header = block.header();

        // Check that `parent_hash` is correct.
        let n = *header.number();
        assert!(
            n > BlockNumberFor::<System>::zero()
                && <frame_system::Pallet<System>>::block_hash(n - BlockNumberFor::<System>::one())
                    == *header.parent_hash(),
            "Parent hash should be valid.",
        );

        match System::ensure_inherents_are_first(block) {
            Ok(num) => num,
            Err(i) => panic!("Invalid inherent position for extrinsic at index {}", i),
        }
    }

    /// Actually execute all transitions for `block`.
    pub fn execute_block(block: Block) {
        sp_io::init_tracing();
        sp_tracing::within_span! {
            sp_tracing::info_span!("execute_block", ?block);
            // Execute `on_runtime_upgrade` and `on_initialize`.
            let mode = Self::initialize_block(block.header());
            let num_inherents = Self::initial_checks(&block) as usize;
            let (header, extrinsics) = block.deconstruct();
            let num_extrinsics = extrinsics.len();

            if mode == ExtrinsicInclusionMode::OnlyInherents && num_extrinsics > num_inherents {
                // Invalid block
                panic!("Only inherents are allowed in this block")
            }

            Self::apply_extrinsics(extrinsics.into_iter());

            // In this case there were no transactions to trigger this state transition:
            if !<frame_system::Pallet<System>>::inherents_applied() {
                defensive_assert!(num_inherents == num_extrinsics);
                Self::inherents_applied();
            }

            <frame_system::Pallet<System>>::note_finished_extrinsics();
            <System as frame_system::Config>::PostTransactions::post_transactions();

            Self::on_idle_hook(*header.number());
            Self::on_finalize_hook(*header.number());
            Self::final_checks(&header, true, true);
        }
    }

    /// Actually execute all transitions for `block`.
    pub fn execute_block_without_state_root_check(block: Block) {
        sp_io::init_tracing();
        sp_tracing::within_span! {
            sp_tracing::info_span!("execute_block_without_state_root_check", ?block);

            #[cfg(feature = "std")]
            let now = std::time::Instant::now();

            // Execute `on_runtime_upgrade` and `on_initialize`.
            Self::initialize_block(block.header());

            #[cfg(feature = "std")]
            let initialize_block_duration = now.elapsed().as_nanos();

            let num_inherents = Self::initial_checks(&block) as usize;
            let (header, extrinsics) = block.deconstruct();
            let num_extrinsics = extrinsics.len();

            #[cfg(feature = "std")]
            let now = std::time::Instant::now();

            Self::apply_extrinsics(extrinsics.into_iter());

            #[cfg(feature = "std")]
            let apply_extrinsics_duration = now.elapsed().as_nanos();

            #[cfg(feature = "std")]
            let now = std::time::Instant::now();

            // In this case there were no transactions to trigger this state transition:
            if !<frame_system::Pallet<System>>::inherents_applied() {
                defensive_assert!(num_inherents == num_extrinsics);
                Self::inherents_applied();
            }

            <frame_system::Pallet<System>>::note_finished_extrinsics();

            Self::on_finalize_hook(*header.number());
            Self::final_checks(&header, false, false);

            #[cfg(feature = "std")]
            let finalize_block_duration = now.elapsed().as_nanos();

            #[cfg(feature = "std")]
            log::debug!(
                target: "runtime::executive",
                "[execute_block] \
                initialize_block: {initialize_block_duration:?}ns, \
                apply_extrinsics: {apply_extrinsics_duration}ns, \
                finalize_block: {finalize_block_duration}ns",
            );
        }
    }

    pub fn finalize_block_without_checks(header: frame_system::pallet_prelude::HeaderFor<System>) {
        // In this case there were no transactions to trigger this state transition:
        if !<frame_system::Pallet<System>>::inherents_applied() {
            // defensive_assert!(num_inherents == num_extrinsics);
            Self::inherents_applied();
        }

        <frame_system::Pallet<System>>::note_finished_extrinsics();

        Self::on_finalize_hook(*header.number());
        Self::final_checks(&header, false, false);
    }

    /// Logic that runs directly after inherent application.
    ///
    /// It advances the Multi-Block-Migrations or runs the `on_poll` hook.
    pub fn inherents_applied() {
        <frame_system::Pallet<System>>::note_inherents_applied();
        <System as frame_system::Config>::PostInherents::post_inherents();

        if <System as frame_system::Config>::MultiBlockMigrator::ongoing() {
            let used_weight = <System as frame_system::Config>::MultiBlockMigrator::step();
            <frame_system::Pallet<System>>::register_extra_weight_unchecked(
                used_weight,
                DispatchClass::Mandatory,
            );
        } else {
            let block_number = <frame_system::Pallet<System>>::block_number();
            Self::on_poll_hook(block_number);
        }
    }

    /// Execute given extrinsics.
    fn apply_extrinsics(extrinsics: impl Iterator<Item = Block::Extrinsic>) {
        extrinsics.into_iter().for_each(|e| {
            if let Err(e) = Self::apply_extrinsic(e) {
                let err: &'static str = e.into();
                panic!("{}", err)
            }
        });
    }

    /// Run the `on_idle` hook of all pallet, but only if there is weight remaining and there are no
    /// ongoing MBMs.
    fn on_idle_hook(block_number: NumberFor<Block>) {
        if <System as frame_system::Config>::MultiBlockMigrator::ongoing() {
            return;
        }

        let weight = <frame_system::Pallet<System>>::block_weight();
        let max_weight = <System::BlockWeights as frame_support::traits::Get<_>>::get().max_block;
        let remaining_weight = max_weight.saturating_sub(weight.total());

        if remaining_weight.all_gt(Weight::zero()) {
            let used_weight = <AllPalletsWithSystem as OnIdle<BlockNumberFor<System>>>::on_idle(
                block_number,
                remaining_weight,
            );
            <frame_system::Pallet<System>>::register_extra_weight_unchecked(
                used_weight,
                DispatchClass::Mandatory,
            );
        }
    }

    fn on_poll_hook(block_number: NumberFor<Block>) {
        defensive_assert!(
            !<System as frame_system::Config>::MultiBlockMigrator::ongoing(),
            "on_poll should not be called during migrations"
        );

        let weight = <frame_system::Pallet<System>>::block_weight();
        let max_weight = <System::BlockWeights as frame_support::traits::Get<_>>::get().max_block;
        let remaining = max_weight.saturating_sub(weight.total());

        if remaining.all_gt(Weight::zero()) {
            let mut meter = WeightMeter::with_limit(remaining);
            <AllPalletsWithSystem as OnPoll<BlockNumberFor<System>>>::on_poll(
                block_number,
                &mut meter,
            );
            <frame_system::Pallet<System>>::register_extra_weight_unchecked(
                meter.consumed(),
                DispatchClass::Mandatory,
            );
        }
    }

    /// Run the `on_finalize` hook of all pallet.
    fn on_finalize_hook(block_number: NumberFor<Block>) {
        <AllPalletsWithSystem as OnFinalize<BlockNumberFor<System>>>::on_finalize(block_number);
    }

    /// Apply extrinsic outside of the block execution function.
    ///
    /// This doesn't attempt to validate anything regarding the block, but it builds a list of uxt
    /// hashes.
    pub fn apply_extrinsic(uxt: Block::Extrinsic) -> ApplyExtrinsicResult {
        sp_io::init_tracing();
        let encoded = uxt.encode();
        let encoded_len = encoded.len();
        sp_tracing::enter_span!(sp_tracing::info_span!("apply_extrinsic",
				ext=?sp_core::hexdisplay::HexDisplay::from(&encoded)));

        // We use the dedicated `is_inherent` check here, since just relying on `Mandatory` dispatch
        // class does not capture optional inherents.
        let is_inherent = System::is_inherent(&uxt);

        // Verify that the signature is good.
        let xt = uxt.check(&Default::default())?;
        let dispatch_info = xt.get_dispatch_info();

        if !is_inherent && !<frame_system::Pallet<System>>::inherents_applied() {
            Self::inherents_applied();
        }

        // We don't need to make sure to `note_extrinsic` only after we know it's going to be
        // executed to prevent it from leaking in storage since at this point, it will either
        // execute or panic (and revert storage changes).
        <frame_system::Pallet<System>>::note_extrinsic(encoded);

        // AUDIT: Under no circumstances may this function panic from here onwards.

        let r = Applyable::apply::<UnsignedValidator>(xt, &dispatch_info, encoded_len)?;

        // Mandatory(inherents) are not allowed to fail.
        //
        // The entire block should be discarded if an inherent fails to apply. Otherwise
        // it may open an attack vector.
        if r.is_err() && dispatch_info.class == DispatchClass::Mandatory {
            return Err(InvalidTransaction::BadMandatory.into());
        }

        let tx_weight = frame_support::dispatch::extract_actual_weight(&r, &dispatch_info);

        <frame_system::Pallet<System>>::note_applied_extrinsic(&r, dispatch_info);

        <frame_system::Pallet<System>>::register_extra_weight_unchecked(
            tx_weight,
            DispatchClass::Normal,
        );

        Ok(r.map(|_| ()).map_err(|e| e.error))
    }

    fn final_checks(
        header: &frame_system::pallet_prelude::HeaderFor<System>,
        check_storage_root: bool,
        check_extrinsics_root: bool,
    ) {
        sp_tracing::enter_span!(sp_tracing::Level::TRACE, "final_checks");
        // remove temporaries
        let new_header = <frame_system::Pallet<System>>::finalize();

        // check digest
        assert_eq!(
            header.digest().logs().len(),
            new_header.digest().logs().len(),
            "Number of digest items must match that calculated."
        );
        let items_zip = header
            .digest()
            .logs()
            .iter()
            .zip(new_header.digest().logs().iter());
        for (header_item, computed_item) in items_zip {
            header_item.check_equal(computed_item);
            assert!(
                header_item == computed_item,
                "Digest item must match that calculated."
            );
        }

        // check storage root.
        if check_storage_root {
            let storage_root = new_header.state_root();
            header.state_root().check_equal(storage_root);
            assert!(
                header.state_root() == storage_root,
                "Storage root must match that calculated."
            );
        }

        if check_extrinsics_root {
            assert!(
                header.extrinsics_root() == new_header.extrinsics_root(),
                "Transaction trie root must be valid.",
            );
        }
    }

    /// Check a given signed transaction for validity. This doesn't execute any
    /// side-effects; it merely checks whether the transaction would panic if it were included or
    /// not.
    ///
    /// Changes made to storage should be discarded.
    pub fn validate_transaction(
        source: TransactionSource,
        uxt: Block::Extrinsic,
        block_hash: Block::Hash,
    ) -> TransactionValidity {
        sp_io::init_tracing();
        use sp_tracing::{enter_span, within_span};

        <frame_system::Pallet<System>>::initialize(
            &(frame_system::Pallet::<System>::block_number() + One::one()),
            &block_hash,
            &Default::default(),
        );

        enter_span! { sp_tracing::Level::TRACE, "validate_transaction" };

        let encoded_len = within_span! { sp_tracing::Level::TRACE, "using_encoded";
            uxt.using_encoded(|d| d.len())
        };

        let xt = within_span! { sp_tracing::Level::TRACE, "check";
            uxt.check(&Default::default())
        }?;

        let dispatch_info = within_span! { sp_tracing::Level::TRACE, "dispatch_info";
            xt.get_dispatch_info()
        };

        if dispatch_info.class == DispatchClass::Mandatory {
            return Err(InvalidTransaction::MandatoryValidation.into());
        }

        within_span! {
            sp_tracing::Level::TRACE, "validate";
            xt.validate::<UnsignedValidator>(source, &dispatch_info, encoded_len)
        }
    }
}
