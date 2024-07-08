use crate::verification::Coin;
use async_trait::async_trait;
use bitcoin::OutPoint;
use sc_client_api::{AuxStore, Backend, BlockBackend, HeaderBackend, StorageProvider};
use sc_consensus::{BlockImport, BlockImportParams, ImportResult, StateAction, StorageChanges};
use sp_api::{ApiExt, CallApiAt, CallContext, Core, ProvideRuntimeApi};
use sp_runtime::traits::{Block as BlockT, HashingFor, Header as HeaderT};
use sp_state_machine::{StorageKey, StorageValue};
use std::marker::PhantomData;
use std::sync::Arc;
use subcoin_primitives::runtime::Subcoin;
use subcoin_primitives::{BitcoinTransactionAdapter, CoinStorageKey};

/// A simply way to track the overall execution info for optimization purpose.
#[derive(Debug, Default)]
pub struct ExecutionInfo {
    /// Time taken by `runtime_api.execute_block` in nanoseconds.
    pub execute_block: u128,
    /// Time taken by `client.state_at` in nanoseconds.
    pub fetch_state: u128,
    /// Time taken by `runtime_api.into_storage_changes` in nanoseconds.
    pub into_storage_changes: u128,
}

impl ExecutionInfo {
    /// Returns the total execution time in nanoseconds.
    pub fn total(&self) -> u128 {
        self.execute_block + self.fetch_state + self.into_storage_changes
    }
}

/// Result of executing a new block.
pub struct ExecuteBlockResult<Block: BlockT> {
    /// New state root for the new block.
    pub state_root: Block::Hash,
    /// Storage changes for the new block.
    pub storage_changes: sp_state_machine::StorageChanges<HashingFor<Block>>,
    /// Execution informantion for performance tracking.
    pub exec_info: ExecutionInfo,
}

/// Represents the state backend storage type for block execution.
#[derive(Debug, Clone, Copy)]
pub enum ExecutionBackend {
    /// Disk backend.
    Disk,
    /// In memory backend.
    InMemory,
}

/// Represents the different strategies for executing a block.
#[derive(Debug, Clone, Copy)]
pub enum BlockExecutionStrategy {
    /// Executes the block using the runtime api `execute_block`,
    RuntimeExecution(ExecutionBackend),
    /// Executes the block without using the runtime api `execute_block`.
    OffRuntimeExecution(ExecutionBackend),
    /// Hybrid strategy combining both disk and in-memory runtime execution for performance comparison.
    BenchmarkRuntimeExecution,
    /// Benchmark all supported strategies.
    ///
    /// Check out the log for the performance details.
    BenchmarkAll,
}

impl BlockExecutionStrategy {
    pub fn runtime_disk() -> Self {
        Self::RuntimeExecution(ExecutionBackend::Disk)
    }

    pub fn off_runtime_in_memory() -> Self {
        Self::OffRuntimeExecution(ExecutionBackend::InMemory)
    }

    /// Returns `true` if the strategy makes use of the in memory backend.
    pub fn in_memory_backend_used(&self) -> bool {
        match self {
            BlockExecutionStrategy::RuntimeExecution(exec_backend)
            | BlockExecutionStrategy::OffRuntimeExecution(exec_backend) => {
                matches!(exec_backend, ExecutionBackend::InMemory)
            }
            BlockExecutionStrategy::BenchmarkRuntimeExecution
            | BlockExecutionStrategy::BenchmarkAll => true,
        }
    }
}

/// Trait for executing and importing the block.
#[async_trait]
pub trait BlockExecutor<Block: BlockT>: Send + Sync {
    /// Returns the type of block execution strategy used.
    fn execution_strategy(&self) -> BlockExecutionStrategy;

    /// Executes the given block on top of the state specified by the parent hash.
    fn execute_block(
        &self,
        parent_hash: Block::Hash,
        block: Block,
    ) -> sp_blockchain::Result<ExecuteBlockResult<Block>>;

    /// Determines whether the block should be imported in the executor.
    ///
    /// `import_block` only makes sense for the executor using in memory backend.
    fn is_in_memory_backend_used(&self) -> bool {
        self.execution_strategy().in_memory_backend_used()
    }

    /// Imports the block using the given import params.
    async fn import_block(
        &mut self,
        import_params: BlockImportParams<Block>,
    ) -> Result<ImportResult, sp_consensus::Error>;
}

/// Backend type for the execution client.
///
/// The process of executing a block has been decoupled from the block import pipeline.
/// There are two kinds of clients for executing the block:
///
/// 1) Client using the disk backend. This is the default behaviour in Substrate. The state
/// is maintained in the disk backend and pruned according to the parameters provided on startup.
///
/// 2) Client using the in memory backend. This is specifically designated for fast block execution
/// in the initial full sync stage, the in memory backend keeps the entire latest chain state in
/// the memory for executing blocks, the previous states are not stored. The block executor using
/// the in memory backend needs to import the block within the executor to update the in memory
/// chain state.
pub enum ClientContext<BI> {
    Disk,
    InMemory(BI),
}

impl<BI> std::fmt::Debug for ClientContext<BI> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disk => write!(f, "Disk"),
            Self::InMemory(_) => write!(f, "InMemory"),
        }
    }
}

impl<BI> ClientContext<BI> {
    pub fn execution_backend(&self) -> ExecutionBackend {
        match self {
            Self::Disk => ExecutionBackend::Disk,
            Self::InMemory(_) => ExecutionBackend::InMemory,
        }
    }
}

/// Block executor using the runtime api `execute_block`.
///
/// This is the standard Substrate block executor.
pub struct RuntimeBlockExecutor<Block, Client, BE, BI> {
    client: Arc<Client>,
    client_context: ClientContext<BI>,
    _phantom: PhantomData<(Block, BE)>,
}

impl<Block, Client, BE, BI> RuntimeBlockExecutor<Block, Client, BE, BI> {
    /// Constructs a new instance of [`RuntimeBlockExecutor`].
    pub fn new(client: Arc<Client>, client_context: ClientContext<BI>) -> Self {
        Self {
            client,
            client_context,
            _phantom: Default::default(),
        }
    }
}

#[async_trait]
impl<Block, BE, Client, BI> BlockExecutor<Block> for RuntimeBlockExecutor<Block, Client, BE, BI>
where
    Block: BlockT,
    BE: Backend<Block>,
    Client: HeaderBackend<Block>
        + BlockBackend<Block>
        + AuxStore
        + ProvideRuntimeApi<Block>
        + StorageProvider<Block, BE>
        + CallApiAt<Block>,
    Client::Api: Core<Block> + Subcoin<Block>,
    BI: BlockImport<Block> + Send + Sync,
{
    fn execution_strategy(&self) -> BlockExecutionStrategy {
        BlockExecutionStrategy::RuntimeExecution(self.client_context.execution_backend())
    }

    fn execute_block(
        &self,
        parent_hash: Block::Hash,
        block: Block,
    ) -> sp_blockchain::Result<ExecuteBlockResult<Block>> {
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

        let state_root = storage_changes.transaction_storage_root;

        Ok(ExecuteBlockResult {
            state_root,
            storage_changes,
            exec_info,
        })
    }

    async fn import_block(
        &mut self,
        import_params: BlockImportParams<Block>,
    ) -> Result<ImportResult, sp_consensus::Error> {
        match &mut self.client_context {
            ClientContext::InMemory(block_import) => block_import
                .import_block(import_params)
                .await
                .map_err(|err| sp_consensus::Error::ClientImport(err.to_string())),
            ClientContext::Disk => {
                unreachable!("not needed for RuntimeBlockExecutor on disk backend");
            }
        }
    }
}

/// Block executor using custom `apply_extrinsics`, for the initial sync process.
pub struct OffRuntimeBlockExecutor<Block, Client, BE, TransactionAdapter, BI> {
    client: Arc<Client>,
    client_context: ClientContext<BI>,
    coin_storage_key: Arc<dyn CoinStorageKey>,
    _phantom: PhantomData<(Block, BE, TransactionAdapter)>,
}

impl<Block, Client, BE, TransactionAdapter, BI>
    OffRuntimeBlockExecutor<Block, Client, BE, TransactionAdapter, BI>
{
    /// Constructs a new instance of [`OffRuntimeBlockExecutor`].
    pub fn new(
        client: Arc<Client>,
        client_context: ClientContext<BI>,
        coin_storage_key: Arc<dyn CoinStorageKey>,
    ) -> Self {
        Self {
            client,
            client_context,
            coin_storage_key,
            _phantom: Default::default(),
        }
    }
}

type StorageEntry = (StorageKey, Option<StorageValue>);

#[allow(unused)]
fn execute_block_off_runtime<Block: BlockT>(
    header: &<Block as BlockT>::Header,
) -> Vec<StorageEntry> {
    use hex_literal::hex;
    use sp_core::Encode;

    let number = header.number();
    let parent_hash = header.parent_hash();
    let digest = header.digest();
    vec![
        // Number<T>
        (
            hex!["26aa394eea5630e07c48ae0c9558cef702a5c1b19ab7a04f536c519aca4983ac"].to_vec(),
            Some(number.encode()),
        ),
        // ParentHash<T>
        (
            hex!["26aa394eea5630e07c48ae0c9558cef78a42f33323cb5ced3b44dd825fda9fcc"].to_vec(),
            Some(parent_hash.encode()),
        ),
        // Digest<T>
        (
            hex!["26aa394eea5630e07c48ae0c9558cef799e7f93fc6a98f0874fd057f111c4d2d"].to_vec(),
            Some(digest.encode()),
        ),
        // BlockWeight<T>
        (
            hex!["26aa394eea5630e07c48ae0c9558cef734abf5cb34d6244378cddbf18e849d96"].to_vec(),
            None,
        ),
    ]
    // Primarily apply_extrinsics()
    //
    // initialize() and finalize() are mostly for generating the header.
    //
    // Number<T>
    // ParentHash<T>
    // Digest<T>
    // BlockHash<T> (parent_number, parent_hash)
    //
    // BlockWeight<T>: always None, as we delete `register_weight_unchecked` within `initialize()`.
}

fn apply_extrinsics_off_runtime<
    Block: BlockT,
    TransactionAdapter: BitcoinTransactionAdapter<Block>,
>(
    extrinsics: Vec<Block::Extrinsic>,
    coin_storage_key: &Arc<dyn CoinStorageKey>,
) -> Vec<(Vec<StorageEntry>, Option<u32>)> {
    use codec::Encode;

    extrinsics
        .iter()
        .enumerate()
        .map(|(index, extrinsic)| {
            let tx = TransactionAdapter::extrinsic_to_bitcoin_transaction(extrinsic);

            let mut changes = Vec::with_capacity(tx.input.len() + tx.output.len());

            for input in &tx.input {
                let OutPoint { txid, vout } = input.previous_output;
                let storage_key = coin_storage_key.storage_key(txid, vout);
                changes.push((storage_key, None));
            }

            let txid = tx.compute_txid();
            let is_coinbase = tx.is_coinbase();

            for (index, txout) in tx.output.into_iter().enumerate() {
                let storage_key = coin_storage_key.storage_key(txid, index as u32);
                let coin = Coin {
                    is_coinbase,
                    amount: txout.value.to_sat(),
                    script_pubkey: txout.script_pubkey.into_bytes(),
                };

                changes.push((storage_key, Some(coin.encode())));
            }

            (changes, Some(index as u32))
        })
        .collect()
}

fn format_time(nanoseconds: u128) -> String {
    const NANOS_PER_MICRO: u128 = 1_000;
    const NANOS_PER_MILLI: u128 = 1_000_000;
    const NANOS_PER_SEC: u128 = 1_000_000_000;

    if nanoseconds >= NANOS_PER_SEC {
        let seconds = nanoseconds as f64 / NANOS_PER_SEC as f64;
        format!("{:.2} s", seconds)
    } else if nanoseconds >= NANOS_PER_MILLI {
        let millis = nanoseconds as f64 / NANOS_PER_MILLI as f64;
        format!("{:.2} ms", millis)
    } else if nanoseconds >= NANOS_PER_MICRO {
        let micros = nanoseconds as f64 / NANOS_PER_MICRO as f64;
        format!("{:.2} µs", micros)
    } else {
        format!("{} ns", nanoseconds)
    }
}

#[derive(Debug, Default)]
struct ExecuteBlockDetails {
    /// initialize_block
    pre: u128,
    /// apply_extrinsics
    apply: u128,
    set_changes: u128,
    /// post_extrinsics
    post: u128,
}

#[async_trait]
impl<Block, BE, Client, TransactionAdapter, BI> BlockExecutor<Block>
    for OffRuntimeBlockExecutor<Block, Client, BE, TransactionAdapter, BI>
where
    Block: BlockT,
    BE: Backend<Block>,
    Client: HeaderBackend<Block>
        + BlockBackend<Block>
        + AuxStore
        + ProvideRuntimeApi<Block>
        + StorageProvider<Block, BE>
        + CallApiAt<Block>,
    Client::Api: Core<Block> + Subcoin<Block>,
    TransactionAdapter: BitcoinTransactionAdapter<Block> + Send + Sync,
    BI: BlockImport<Block> + Send + Sync + 'static,
{
    fn execution_strategy(&self) -> BlockExecutionStrategy {
        BlockExecutionStrategy::OffRuntimeExecution(self.client_context.execution_backend())
    }

    fn execute_block(
        &self,
        parent_hash: Block::Hash,
        block: Block,
    ) -> sp_blockchain::Result<ExecuteBlockResult<Block>> {
        let mut runtime_api = self.client.runtime_api();
        runtime_api.set_call_context(CallContext::Onchain);

        let (header, extrinsics) = block.deconstruct();

        let mut exec_info = ExecutionInfo::default();

        let now = std::time::Instant::now();

        let mut exec_details = ExecuteBlockDetails::default();
        let t = std::time::Instant::now();
        runtime_api.initialize_block(parent_hash, &header)?;
        exec_details.pre = t.elapsed().as_nanos();

        let t = std::time::Instant::now();
        let block_storage_changes = apply_extrinsics_off_runtime::<Block, TransactionAdapter>(
            extrinsics,
            &self.coin_storage_key,
        );
        exec_details.apply = t.elapsed().as_nanos();

        // block_storage_changes.push((execute_block_off_runtime::<Block>(&header), None));

        let t = std::time::Instant::now();
        runtime_api
            .changes_mut()
            .borrow_mut()
            .set_extrinsic_storage_changes(block_storage_changes);
        exec_details.set_changes = t.elapsed().as_nanos();

        let t = std::time::Instant::now();
        runtime_api.finalize_block_without_checks(parent_hash, header)?;
        exec_details.post = t.elapsed().as_nanos();

        tracing::debug!("off_runtime({:?}): {exec_details:?}", self.client_context);

        exec_info.execute_block = now.elapsed().as_nanos();

        let now = std::time::Instant::now();
        let state = self.client.state_at(parent_hash)?;
        exec_info.fetch_state = now.elapsed().as_nanos();

        let now = std::time::Instant::now();
        let storage_changes = runtime_api
            .into_storage_changes(&state, parent_hash)
            .map_err(sp_blockchain::Error::StorageChanges)?;
        exec_info.into_storage_changes = now.elapsed().as_nanos();

        tracing::debug!(
            "off_runtime({:?}): {exec_info:?}, total: {}",
            self.client_context,
            format_time(exec_info.total()),
        );

        let state_root = storage_changes.transaction_storage_root;

        Ok(ExecuteBlockResult {
            state_root,
            storage_changes,
            exec_info,
        })
    }

    async fn import_block(
        &mut self,
        import_params: BlockImportParams<Block>,
    ) -> Result<ImportResult, sp_consensus::Error> {
        match &mut self.client_context {
            ClientContext::InMemory(block_import) => block_import
                .import_block(import_params)
                .await
                .map_err(|err| sp_consensus::Error::ClientImport(err.to_string())),
            ClientContext::Disk => {
                unreachable!("Not needed in disk backend context")
            }
        }
    }
}

pub struct BenchmarkRuntimeBlockExecutor<Block: BlockT> {
    disk_runtime_block_executor: Box<dyn BlockExecutor<Block>>,
    in_memory_runtime_block_executor: Box<dyn BlockExecutor<Block>>,
}

impl<Block: BlockT> BenchmarkRuntimeBlockExecutor<Block> {
    pub fn new(
        disk_runtime_block_executor: Box<dyn BlockExecutor<Block>>,
        in_memory_runtime_block_executor: Box<dyn BlockExecutor<Block>>,
    ) -> Self {
        Self {
            disk_runtime_block_executor,
            in_memory_runtime_block_executor,
        }
    }
}

#[async_trait]
impl<Block: BlockT> BlockExecutor<Block> for BenchmarkRuntimeBlockExecutor<Block> {
    fn execution_strategy(&self) -> BlockExecutionStrategy {
        BlockExecutionStrategy::BenchmarkRuntimeExecution
    }

    fn execute_block(
        &self,
        parent_hash: Block::Hash,
        block: Block,
    ) -> sp_blockchain::Result<ExecuteBlockResult<Block>> {
        tracing::debug!("============================================ In Memory Executor");
        let ExecuteBlockResult {
            state_root: in_memory_state_root,
            storage_changes: _,
            exec_info: in_memory_runtime_exec_info,
        } = self
            .in_memory_runtime_block_executor
            .execute_block(parent_hash, block.clone())?;

        tracing::debug!("============================================ Disk Executor");
        let ExecuteBlockResult {
            state_root,
            storage_changes,
            exec_info,
        } = self
            .disk_runtime_block_executor
            .execute_block(parent_hash, block)?;

        assert_eq!(in_memory_state_root, state_root);

        let in_memory_runtime_total = in_memory_runtime_exec_info.total();
        tracing::debug!(
            "in_memory: {:?}, total: {in_memory_runtime_total}",
            in_memory_runtime_exec_info
        );
        let disk_runtime_total = exec_info.total();
        tracing::debug!(
            "     disk: {:?}, total: {disk_runtime_total}, time (disk/in_memory): {:.2}x",
            exec_info,
            disk_runtime_total as f64 / in_memory_runtime_total as f64
        );

        Ok(ExecuteBlockResult {
            state_root,
            storage_changes,
            exec_info,
        })
    }

    async fn import_block(
        &mut self,
        import_params: BlockImportParams<Block>,
    ) -> Result<ImportResult, sp_consensus::Error> {
        self.in_memory_runtime_block_executor
            .import_block(import_params)
            .await
            .map_err(|err| sp_consensus::Error::ClientImport(err.to_string()))
    }
}

pub struct BenchmarkAllExecutor<
    Block,
    DiskRuntime,
    InMemoryRuntime,
    DiskOffRuntime,
    InMemoryOffRuntime,
> {
    disk_runtime_block_executor: DiskRuntime,
    in_memory_runtime_block_executor: InMemoryRuntime,
    disk_off_runtime_block_executor: DiskOffRuntime,
    in_memory_off_runtime_block_executor: InMemoryOffRuntime,
    _phantom: PhantomData<Block>,
}

impl<Block, DiskRuntime, InMemoryRuntime, DiskOffRuntime, InMemoryOffRuntime>
    BenchmarkAllExecutor<Block, DiskRuntime, InMemoryRuntime, DiskOffRuntime, InMemoryOffRuntime>
{
    pub fn new(
        disk_runtime_block_executor: DiskRuntime,
        in_memory_runtime_block_executor: InMemoryRuntime,
        disk_off_runtime_block_executor: DiskOffRuntime,
        in_memory_off_runtime_block_executor: InMemoryOffRuntime,
    ) -> Self {
        Self {
            disk_runtime_block_executor,
            in_memory_runtime_block_executor,
            disk_off_runtime_block_executor,
            in_memory_off_runtime_block_executor,
            _phantom: Default::default(),
        }
    }
}

fn display_main_changes(changes: &[(Vec<u8>, Option<Vec<u8>>)]) -> String {
    changes
        .iter()
        .map(|(key, value)| format!("{}, {value:?}", sp_core::hexdisplay::HexDisplay::from(key)))
        .collect::<Vec<String>>()
        .join("\n")
}

#[async_trait]
impl<Block, DiskRuntime, InMemoryRuntime, DiskOffRuntime, InMemoryOffRuntime> BlockExecutor<Block>
    for BenchmarkAllExecutor<
        Block,
        DiskRuntime,
        InMemoryRuntime,
        DiskOffRuntime,
        InMemoryOffRuntime,
    >
where
    Block: BlockT,
    DiskRuntime: BlockExecutor<Block>,
    InMemoryRuntime: BlockExecutor<Block>,
    DiskOffRuntime: BlockExecutor<Block>,
    InMemoryOffRuntime: BlockExecutor<Block>,
{
    fn execution_strategy(&self) -> BlockExecutionStrategy {
        BlockExecutionStrategy::BenchmarkAll
    }

    fn execute_block(
        &self,
        parent_hash: Block::Hash,
        block: Block,
    ) -> sp_blockchain::Result<ExecuteBlockResult<Block>> {
        let ExecuteBlockResult {
            state_root: in_memory_state_root,
            storage_changes: c1,
            exec_info: in_memory_runtime_exec_info,
        } = self
            .in_memory_runtime_block_executor
            .execute_block(parent_hash, block.clone())?;

        let ExecuteBlockResult {
            state_root: in_memory_off_runtime_state_root,
            storage_changes: c2,
            exec_info: in_memory_off_runtime_exec_info,
        } = self
            .in_memory_off_runtime_block_executor
            .execute_block(parent_hash, block.clone())?;

        if in_memory_state_root != in_memory_off_runtime_state_root {
            tracing::debug!(
                "    runtime changes: \n{}",
                display_main_changes(&c1.main_storage_changes)
            );
            tracing::debug!(
                "off_runtime changes: \n{}",
                display_main_changes(&c2.main_storage_changes)
            );
            panic!("Off runtime state root does not match the runtime state root");
        }

        let ExecuteBlockResult {
            state_root: _disk_off_runtime_state_root,
            storage_changes: _,
            exec_info: disk_off_runtime_exec_info,
        } = self
            .disk_off_runtime_block_executor
            .execute_block(parent_hash, block.clone())?;

        let ExecuteBlockResult {
            state_root,
            storage_changes,
            exec_info: disk_runtime_exec_info,
        } = self
            .disk_runtime_block_executor
            .execute_block(parent_hash, block)?;

        // FIXME
        // assert_eq!(in_memory_state_root, state_root);

        let in_memory_runtime_total = in_memory_runtime_exec_info.total();
        let in_memory_off_runtime_total = in_memory_off_runtime_exec_info.total();
        let disk_off_runtime_total = disk_off_runtime_exec_info.total();
        let disk_runtime_total = disk_runtime_exec_info.total();

        let min_total = in_memory_runtime_total
            .min(disk_off_runtime_total)
            .min(in_memory_off_runtime_total)
            .min(disk_runtime_total);

        tracing::debug!(
            "    runtime(in_memory): time (relative to min): {:.2}x, total: {in_memory_runtime_total}, {in_memory_runtime_exec_info:?}",
            in_memory_runtime_total as f64 / min_total as f64
        );
        tracing::debug!(
            "off_runtime(in_memory): time (relative to min): {:.2}x, total: {in_memory_off_runtime_total}, {in_memory_off_runtime_exec_info:?}",
            in_memory_off_runtime_total as f64 / min_total as f64
        );
        tracing::debug!(
            "     off_runtime(disk): time (relative to min): {:.2}x, total: {disk_off_runtime_total}, {disk_off_runtime_exec_info:?}",
            disk_off_runtime_total as f64 / min_total as f64
        );
        tracing::debug!(
            "         runtime(disk): time (relative to min): {:.2}x, total: {disk_runtime_total}, {disk_runtime_exec_info:?}",
            disk_runtime_total as f64 / min_total as f64
        );

        Ok(ExecuteBlockResult {
            state_root,
            storage_changes,
            exec_info: disk_runtime_exec_info,
        })
    }

    async fn import_block(
        &mut self,
        import_params: BlockImportParams<Block>,
    ) -> Result<ImportResult, sp_consensus::Error> {
        let mut params = BlockImportParams::new(import_params.origin, import_params.header.clone());
        params.post_digests.clone_from(&import_params.post_digests);
        params.state_action = match &import_params.state_action {
            StateAction::ApplyChanges(StorageChanges::<Block>::Changes(changes)) => {
                StateAction::ApplyChanges(StorageChanges::<Block>::Changes(
                    crate::block_import::clone_storage_changes::<Block>(changes),
                ))
            }
            _ => unreachable!("Must be ApplyChanges"),
        };
        params
            .auxiliary
            .clone_from(&import_params.auxiliary.clone());
        params.fork_choice = import_params.fork_choice;
        params
            .import_existing
            .clone_from(&import_params.import_existing);
        params.post_hash.clone_from(&import_params.post_hash);

        self.in_memory_off_runtime_block_executor
            .import_block(params)
            .await
            .map_err(|err| sp_consensus::Error::ClientImport(err.to_string()))?;

        self.in_memory_runtime_block_executor
            .import_block(import_params)
            .await
            .map_err(|err| sp_consensus::Error::ClientImport(err.to_string()))
    }
}
