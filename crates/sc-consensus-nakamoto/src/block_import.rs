//! This module provides the implementation for importing Bitcoin blocks into Subcoin.
//!
//! Each Bitcoin block is converted into a Substrate block and imported into the database.
//! The Bitcoin header is included in the Substrate header as a `DigestItem`, each Bitcoin
//! transaction is wrapped into an unsigned extrinsic defined in pallet-bitcoin.
//!
//! Key components:
//!
//! - [`BitcoinBlockImporter`]
//!   The main struct responsible for importing Bitcoin blocks and managing the import process.
//!
//! - [`BitcoinBlockImport`]
//!   An async trait for importing Bitcoin blocks, which is implemented by [`BitcoinBlockImporter`].
//!
//! - [`ImportConfig`]
//!   Configuration for block import, including network type, verification level, and execution options.
//!
//! - [`ImportStatus`]
//!   An enum representing the result of an import operation, with variants for different import outcomes.

use crate::ScriptEngine;
use crate::metrics::Metrics;
use crate::verification::{BlockVerification, BlockVerifier};
use bitcoin::blockdata::block::Header as BitcoinHeader;
use bitcoin::hashes::Hash;
use bitcoin::{Block as BitcoinBlock, BlockHash, Network, Work};
use codec::Encode;
use sc_client_api::{AuxStore, Backend, BlockBackend, HeaderBackend, StorageProvider};
use sc_consensus::{
    BlockImport, BlockImportParams, ForkChoiceStrategy, ImportResult, ImportedAux, StateAction,
    StorageChanges,
};
use sp_api::{ApiExt, CallApiAt, CallContext, Core, ProvideRuntimeApi};
use sp_blockchain::HashAndNumber;
use sp_consensus::{BlockOrigin, BlockStatus};
use sp_runtime::traits::{
    Block as BlockT, Hash as HashT, HashingFor, Header as HeaderT, NumberFor,
};
use sp_runtime::{SaturatedConversion, Saturating};
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Instant;
use subcoin_primitives::runtime::SubcoinApi;
use subcoin_primitives::{
    BackendExt, BitcoinTransactionAdapter, CoinStorageKey, substrate_header_digest,
};
use substrate_prometheus_endpoint::Registry;

/// Block import configuration.
#[derive(Debug, Clone)]
pub struct ImportConfig {
    /// Bitcoin network type.
    pub network: Network,
    /// Specify the block verification level.
    pub block_verification: BlockVerification,
    /// Whether to execute the transactions in the block.
    pub execute_block: bool,
    /// Bitcoin script interpreter.
    pub script_engine: ScriptEngine,
}

#[derive(Debug, Default)]
struct Stats {
    /// Total transactions processed.
    total_txs: usize,
    /// Total blocks processed.
    total_blocks: usize,
    // Total block execution times in milliseconds.
    total_execution_time: usize,
}

impl Stats {
    fn transaction_per_millisecond(&self) -> f64 {
        self.total_txs as f64 / self.total_execution_time as f64
    }

    fn block_per_second(&self) -> f64 {
        self.total_blocks as f64 * 1000f64 / self.total_execution_time as f64
    }

    fn record_new_block_execution<Block: BlockT>(
        &mut self,
        block_number: NumberFor<Block>,
        block_hash: Block::Hash,
        tx_count: usize,
        execution_time: u128,
    ) {
        self.total_txs += tx_count;
        self.total_blocks += 1;
        self.total_execution_time += execution_time as usize;
        let tx_per_ms = self.transaction_per_millisecond();
        let block_per_second = self.block_per_second();

        tracing::debug!(
            "Executed block#{block_number} ({tx_count} txs) ({block_hash}) \
            {tx_per_ms:.2} tx/ms, {block_per_second:.2} block/s, execution time: {execution_time} ms",
        );
    }
}

/// Bitcoin specific block import implementation.
pub struct BitcoinBlockImporter<Block, Client, BE, BI, TransactionAdapter> {
    client: Arc<Client>,
    inner: BI,
    stats: Stats,
    config: ImportConfig,
    verifier: BlockVerifier<Block, Client, BE>,
    metrics: Option<Metrics>,
    last_block_execution_report: Instant,
    _phantom: PhantomData<TransactionAdapter>,
}

impl<Block, Client, BE, BI, TransactionAdapter>
    BitcoinBlockImporter<Block, Client, BE, BI, TransactionAdapter>
where
    Block: BlockT,
    BE: Backend<Block> + 'static,
    Client: HeaderBackend<Block>
        + BlockBackend<Block>
        + AuxStore
        + ProvideRuntimeApi<Block>
        + StorageProvider<Block, BE>
        + CallApiAt<Block>
        + Send
        + 'static,
    Client::Api: Core<Block> + SubcoinApi<Block>,
    BI: BlockImport<Block> + Send + Sync + 'static,
    TransactionAdapter: BitcoinTransactionAdapter<Block>,
{
    /// Constructs a new instance of [`BitcoinBlockImporter`].
    pub fn new(
        client: Arc<Client>,
        block_import: BI,
        config: ImportConfig,
        coin_storage_key: Arc<dyn CoinStorageKey>,
        registry: Option<&Registry>,
    ) -> Self {
        let verifier = BlockVerifier::new(
            client.clone(),
            config.network,
            config.block_verification,
            coin_storage_key,
            config.script_engine,
        );
        let metrics = match registry {
            Some(registry) => Metrics::register(registry)
                .map_err(|err| {
                    tracing::error!("Failed to registry metrics: {err:?}");
                })
                .ok(),
            None => None,
        };
        Self {
            client,
            inner: block_import,
            stats: Stats::default(),
            config,
            verifier,
            metrics,
            last_block_execution_report: Instant::now(),
            _phantom: Default::default(),
        }
    }

    #[inline]
    fn substrate_block_hash(&self, bitcoin_block_hash: BlockHash) -> Option<Block::Hash> {
        BackendExt::<Block>::substrate_block_hash_for(&self.client, bitcoin_block_hash)
    }

    // Returns the corresponding substrate block info for given bitcoin block hash if any.
    fn fetch_substrate_block_info(
        &self,
        bitcoin_block_hash: BlockHash,
    ) -> sp_blockchain::Result<Option<HashAndNumber<Block>>> {
        let Some(substrate_block_hash) = self.substrate_block_hash(bitcoin_block_hash) else {
            return Ok(None);
        };

        let block_number =
            self.client
                .number(substrate_block_hash)?
                .ok_or(sp_blockchain::Error::UnknownBlock(format!(
                    "Substate block hash mapping exists, \
                    but block#{substrate_block_hash} is not in chain"
                )))?;

        Ok(Some(HashAndNumber {
            number: block_number,
            hash: substrate_block_hash,
        }))
    }

    fn execute_block_at(
        &mut self,
        block_number: NumberFor<Block>,
        parent_hash: Block::Hash,
        block: Block,
    ) -> sp_blockchain::Result<(
        Block::Hash,
        sp_state_machine::StorageChanges<HashingFor<Block>>,
    )> {
        let timer = std::time::Instant::now();

        let transactions_count = block.extrinsics().len();
        let block_size = block.encoded_size();

        let mut runtime_api = self.client.runtime_api();
        runtime_api.set_call_context(CallContext::Onchain);

        runtime_api.execute_block_without_state_root_check(parent_hash, block)?;

        let state = self.client.state_at(parent_hash)?;

        let storage_changes = runtime_api
            .into_storage_changes(&state, parent_hash)
            .map_err(sp_blockchain::Error::StorageChanges)?;

        let state_root = storage_changes.transaction_storage_root;

        if let Some(metrics) = &self.metrics {
            const BLOCK_EXECUTION_REPORT_INTERVAL: u128 = 50;

            // Executing blocks before 200000 is pretty fast, it becomes
            // increasingly slower beyond this point, we only cares about
            // the slow block executions.
            if self.last_block_execution_report.elapsed().as_millis()
                > BLOCK_EXECUTION_REPORT_INTERVAL
            {
                let block_number: u32 = block_number.saturated_into();
                let execution_time = timer.elapsed().as_millis();
                metrics.report_block_execution(
                    block_number.saturated_into(),
                    transactions_count,
                    block_size,
                    execution_time,
                );
                self.last_block_execution_report = Instant::now();
            }
        }

        Ok((state_root, storage_changes))
    }

    fn prepare_substrate_block_import(
        &mut self,
        block: BitcoinBlock,
        substrate_parent_block: HashAndNumber<Block>,
        origin: BlockOrigin,
    ) -> sp_blockchain::Result<BlockImportParams<Block>> {
        let HashAndNumber {
            number: parent_block_number,
            hash: parent_hash,
        } = substrate_parent_block;

        let block_number = parent_block_number.saturating_add(1u32.into());

        let extrinsics = block
            .txdata
            .clone()
            .into_iter()
            .map(TransactionAdapter::bitcoin_transaction_into_extrinsic)
            .collect::<Vec<_>>();

        let extrinsics_root = HashingFor::<Block>::ordered_trie_root(
            extrinsics.iter().map(|xt| xt.encode()).collect(),
            sp_core::storage::StateVersion::V0,
        );

        let digest = substrate_header_digest(&block.header);

        // We know everything for the Substrate header except the state root, which will be
        // obtained after executing the block manually.
        let mut header = <<Block as BlockT>::Header as HeaderT>::new(
            block_number,
            extrinsics_root,
            Default::default(),
            parent_hash,
            digest,
        );

        // subcoin bootstrap node must execute every block.
        let state_action = if self.config.execute_block {
            let now = std::time::Instant::now();

            let tx_count = extrinsics.len();

            let (state_root, storage_changes) = self.execute_block_at(
                block_number,
                parent_hash,
                Block::new(header.clone(), extrinsics.clone()),
            )?;

            let execution_time = now.elapsed().as_millis();

            self.stats.record_new_block_execution::<Block>(
                block_number,
                header.hash(),
                tx_count,
                execution_time,
            );

            // Now it's a normal Substrate header after setting the state root.
            header.set_state_root(state_root);

            StateAction::ApplyChanges(StorageChanges::<Block>::Changes(storage_changes))
        } else {
            StateAction::Skip
        };

        let substrate_block_hash = header.hash();
        let bitcoin_block_hash = block.header.block_hash();

        let mut block_import_params = BlockImportParams::new(origin, header);
        let (total_work, fork_choice) = calculate_chain_work_and_fork_choice(
            &self.client,
            &block.header,
            block_number.saturated_into(),
        )?;
        block_import_params.fork_choice = Some(fork_choice);
        block_import_params.body = Some(extrinsics);
        block_import_params.state_action = state_action;

        write_aux_storage(
            &mut block_import_params,
            bitcoin_block_hash,
            substrate_block_hash,
            total_work,
        );

        Ok(block_import_params)
    }
}

pub(crate) fn calculate_chain_work_and_fork_choice<Block, Client>(
    client: &Arc<Client>,
    block_header: &BitcoinHeader,
    block_number: u32,
) -> sp_blockchain::Result<(Work, ForkChoiceStrategy)>
where
    Block: BlockT,
    Client: HeaderBackend<Block> + AuxStore,
{
    let prev_blockhash = block_header.prev_blockhash;

    let parent_work = if block_number == 1u32 {
        // Genesis block's work is hardcoded.
        client
            .block_header(prev_blockhash)
            .expect("Genesis header must exist; qed")
            .work()
    } else {
        crate::aux_schema::load_chain_work(client.as_ref(), prev_blockhash)?
    };

    let total_work = parent_work + block_header.work();

    let fork_choice = {
        let info = client.info();

        let parent_hash = client.substrate_block_hash_for(prev_blockhash).ok_or(
            sp_blockchain::Error::Backend(format!(
                "Missing substrate block hash for #{prev_blockhash}"
            )),
        )?;

        let last_best_work = if info.best_hash == parent_hash {
            parent_work
        } else {
            let bitcoin_block_hash = client
                .bitcoin_block_hash_for(info.best_hash)
                .expect("Best bitcoin hash must exist; qed");
            crate::aux_schema::load_chain_work(client.as_ref(), bitcoin_block_hash)?
        };

        ForkChoiceStrategy::Custom(total_work > last_best_work)
    };

    Ok((total_work, fork_choice))
}

/// Writes storage into the auxiliary data of the `block_import_params`, which
/// will be stored in the aux-db within the [`BitcoinBlockImport::import_block`]
/// or Substrate import queue's verification process.
///
/// The auxiliary data stored includes:
/// - A mapping between a Bitcoin block hash and a Substrate block hash.
/// - Total accumulated proof-of-work up to the given block.
pub(crate) fn write_aux_storage<Block: BlockT>(
    block_import_params: &mut BlockImportParams<Block>,
    bitcoin_block_hash: BlockHash,
    substrate_block_hash: Block::Hash,
    chain_work: Work,
) {
    block_import_params.auxiliary.push((
        bitcoin_block_hash.to_byte_array().to_vec(),
        Some(substrate_block_hash.encode()),
    ));
    crate::aux_schema::write_chain_work(bitcoin_block_hash, chain_work, |(k, v)| {
        block_import_params.auxiliary.push((k, Some(v)))
    });
}

/// Result of the operation of importing a Bitcoin block.
///
/// Same semantic with [`sc_consensus::ImportResult`] with additional information
/// in the [`ImportStatus::Imported`] variant.
#[derive(Debug, Clone)]
pub enum ImportStatus {
    /// Block was imported successfully.
    Imported {
        block_number: u32,
        block_hash: BlockHash,
        aux: ImportedAux,
    },
    /// Already in the blockchain.
    AlreadyInChain(u32),
    /// Block parent is not in the chain.
    UnknownParent,
    /// Parent state is missing.
    MissingState,
    /// Block or parent is known to be bad.
    KnownBad,
}

impl ImportStatus {
    /// Returns `true` if the import status is [`Self::UnknownParent`].
    pub fn is_unknown_parent(&self) -> bool {
        matches!(self, Self::UnknownParent)
    }
}

/// A trait for importing Bitcoin blocks.
#[async_trait::async_trait]
pub trait BitcoinBlockImport: Send + Sync + 'static {
    /// Import a Bitcoin block.
    async fn import_block(
        &mut self,
        block: BitcoinBlock,
        origin: BlockOrigin,
    ) -> Result<ImportStatus, sp_consensus::Error>;
}

#[async_trait::async_trait]
impl<Block, Client, BE, BI, TransactionAdapter> BitcoinBlockImport
    for BitcoinBlockImporter<Block, Client, BE, BI, TransactionAdapter>
where
    Block: BlockT,
    BE: Backend<Block> + Send + Sync + 'static,
    Client: HeaderBackend<Block>
        + BlockBackend<Block>
        + StorageProvider<Block, BE>
        + AuxStore
        + ProvideRuntimeApi<Block>
        + CallApiAt<Block>
        + Send
        + 'static,
    Client::Api: Core<Block> + SubcoinApi<Block>,
    BI: BlockImport<Block> + Send + Sync + 'static,
    TransactionAdapter: BitcoinTransactionAdapter<Block> + Send + Sync + 'static,
{
    async fn import_block(
        &mut self,
        block: BitcoinBlock,
        origin: BlockOrigin,
    ) -> Result<ImportStatus, sp_consensus::Error> {
        if let Some(block_number) = self.client.block_number(block.block_hash()) {
            return Ok(ImportStatus::AlreadyInChain(block_number));
        }

        let bitcoin_parent_hash = block.header.prev_blockhash;

        let import_err = sp_consensus::Error::ClientImport;

        let Some(substrate_parent_block) = self
            .fetch_substrate_block_info(bitcoin_parent_hash)
            .map_err(|err| import_err(err.to_string()))?
        else {
            // The parent block must exist to proceed, otherwise it's an orphan bolck.
            return Ok(ImportStatus::UnknownParent);
        };

        if self.config.execute_block {
            // Ensure the parent block has been imported and the parent state exists.
            match self
                .client
                .block_status(substrate_parent_block.hash)
                .map_err(|err| import_err(err.to_string()))?
            {
                BlockStatus::InChainWithState | BlockStatus::Queued => {}
                BlockStatus::Unknown => return Ok(ImportStatus::UnknownParent),
                BlockStatus::InChainPruned => return Ok(ImportStatus::MissingState),
                BlockStatus::KnownBad => return Ok(ImportStatus::KnownBad),
            }
        }

        let block_number = substrate_parent_block.number.saturated_into::<u32>() + 1u32;
        let block_hash = block.block_hash();

        // Consensus-level Bitcoin block verification.
        self.verifier
            .verify_block(block_number, &block)
            .map_err(|err| import_err(format!("{err:?}")))?;

        let block_import_params = self
            .prepare_substrate_block_import(block, substrate_parent_block, origin)
            .map_err(|err| import_err(err.to_string()))?;

        self.inner
            .import_block(block_import_params)
            .await
            .map(|import_result| match import_result {
                ImportResult::Imported(aux) => ImportStatus::Imported {
                    block_number,
                    block_hash,
                    aux,
                },
                ImportResult::AlreadyInChain => ImportStatus::AlreadyInChain(block_number),
                ImportResult::KnownBad => ImportStatus::KnownBad,
                ImportResult::UnknownParent => ImportStatus::UnknownParent,
                ImportResult::MissingState => ImportStatus::MissingState,
            })
            .map_err(|err| import_err(err.to_string()))
    }
}
