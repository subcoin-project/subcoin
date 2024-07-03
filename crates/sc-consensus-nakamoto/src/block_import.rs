use crate::block_executor::ExecutionInfo;
use crate::verification::{BlockVerification, BlockVerifier};
use bitcoin::hashes::Hash;
use bitcoin::{Block as BitcoinBlock, BlockHash, Network};
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
use subcoin_primitives::runtime::Subcoin;
use subcoin_primitives::{
    substrate_header_digest, BackendExt, BitcoinTransactionAdapter, CoinStorageKey,
};

#[derive(Debug, Clone)]
pub struct ImportConfig {
    /// Bitcoin network type.
    pub network: Network,
    /// Specify the block verification level.
    pub block_verification: BlockVerification,
    /// Whether to execute the transactions in the block.
    pub execute_block: bool,
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
    Client::Api: Core<Block> + Subcoin<Block>,
    BI: BlockImport<Block> + Send + Sync + 'static,
    TransactionAdapter: BitcoinTransactionAdapter<Block>,
{
    /// Constructs a new instance of [`BitcoinBlockImporter`].
    pub fn new(
        client: Arc<Client>,
        block_import: BI,
        config: ImportConfig,
        coin_storage_key: Arc<dyn CoinStorageKey>,
    ) -> Self {
        let verifier = BlockVerifier::new(
            client.clone(),
            config.network,
            config.block_verification,
            coin_storage_key,
        );
        Self {
            client,
            inner: block_import,
            stats: Stats::default(),
            config,
            verifier,
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
        &self,
        parent_hash: Block::Hash,
        block: Block,
    ) -> sp_blockchain::Result<(
        Block::Hash,
        sp_state_machine::StorageChanges<HashingFor<Block>>,
    )> {
        let mut runtime_api = self.client.runtime_api();
        runtime_api.set_call_context(CallContext::Onchain);

        let mut exec_info = ExecutionInfo::default();

        let now = std::time::Instant::now();
        runtime_api.execute_block_without_state_root_check(parent_hash, block)?;
        exec_info.execute_block = now.elapsed().as_nanos();

        let now = std::time::Instant::now();
        let state = self.client.state_at(parent_hash)?;
        exec_info.fetch_state = now.elapsed().as_nanos();

        let now = std::time::Instant::now();
        let storage_changes = runtime_api
            .into_storage_changes(&state, parent_hash)
            .map_err(sp_blockchain::Error::StorageChanges)?;
        exec_info.into_storage_changes = now.elapsed().as_nanos();

        tracing::trace!(
            "Block execution info: {exec_info:?}, total: {}ms",
            exec_info.total()
        );

        let state_root = storage_changes.transaction_storage_root;

        Ok((state_root, storage_changes))
    }

    fn prepare_substrate_block_import(
        &mut self,
        block: BitcoinBlock,
        substrate_parent_block: HashAndNumber<Block>,
    ) -> sp_blockchain::Result<BlockImportParams<Block>> {
        let HashAndNumber {
            number: parent_block_number,
            hash: parent_hash,
        } = substrate_parent_block;

        let block_number = parent_block_number.saturating_add(1u32.into());

        let extrinsics = block
            .txdata
            .iter()
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

            let (state_root, storage_changes) =
                self.execute_block_at(parent_hash, Block::new(header.clone(), extrinsics.clone()))?;

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

        let mut block_import_params = BlockImportParams::new(BlockOrigin::Own, header);
        block_import_params.body = Some(extrinsics);
        block_import_params.fork_choice = Some(ForkChoiceStrategy::LongestChain);
        block_import_params.state_action = state_action;

        // Track the mapping of Bitcoin block hash to the Substrate block hash in aux-db.
        block_import_params.auxiliary = vec![(
            bitcoin_block_hash.to_byte_array().to_vec(),
            Some(substrate_block_hash.encode()),
        )];

        Ok(block_import_params)
    }
}

/// Result of the operation of importing a Bitcoin block.
///
/// Same semantic with [`sc_consensus::ImportResult`] for including more information
/// in the Imported variant.
#[derive(Debug, Clone)]
pub enum ImportStatus {
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
    Client::Api: Core<Block> + Subcoin<Block>,
    BI: BlockImport<Block> + Send + Sync + 'static,
    TransactionAdapter: BitcoinTransactionAdapter<Block> + Send + Sync + 'static,
{
    async fn import_block(
        &mut self,
        block: BitcoinBlock,
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
            .map_err(|err| import_err(err.to_string()))?;

        let block_import_params = self
            .prepare_substrate_block_import(block, substrate_parent_block)
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
