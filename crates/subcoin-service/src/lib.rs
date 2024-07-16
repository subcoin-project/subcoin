//! Subcoin service implementation. Specialized wrapper over substrate service.

#![allow(deprecated)]

mod block_executor;
pub mod chain_spec;
mod genesis_block_builder;
mod transaction_adapter;

use bitcoin::hashes::Hash;
use block_executor::{new_block_executor, new_in_memory_client};
use frame_benchmarking_cli::SUBSTRATE_REFERENCE_HARDWARE;
use futures::StreamExt;
use genesis_block_builder::GenesisBlockBuilder;
use sc_client_api::{AuxStore, BlockchainEvents, Finalizer, HeaderBackend};
use sc_consensus::import_queue::BasicQueue;
use sc_consensus::{BlockImportParams, Verifier};
use sc_consensus_nakamoto::{BlockExecutionStrategy, BlockExecutor};
use sc_executor::NativeElseWasmExecutor;
use sc_network::PeerId;
use sc_service::error::Error as ServiceError;
use sc_service::{Configuration, NativeExecutionDispatch, Role, TaskManager};
use sc_telemetry::{Telemetry, TelemetryWorker};
use sc_utils::mpsc::{tracing_unbounded, TracingUnboundedReceiver, TracingUnboundedSender};
use sp_core::traits::SpawnNamed;
use sp_core::Encode;
use sp_keystore::KeystorePtr;
use sp_runtime::traits::{Block as BlockT, CheckedSub};
use std::ops::Deref;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use subcoin_runtime::interface::OpaqueBlock as Block;
use subcoin_runtime::RuntimeApi;

pub use transaction_adapter::TransactionAdapter;

/// This is a specialization of the general Substrate ChainSpec type.
pub type ChainSpec = sc_service::GenericChainSpec;

/// Disk backend client type.
pub type FullClient =
    sc_service::TFullClient<Block, RuntimeApi, NativeElseWasmExecutor<BitcoinExecutorDispatch>>;
type FullBackend = sc_service::TFullBackend<Block>;
type FullSelectChain = sc_consensus::LongestChain<FullBackend, Block>;

/// In memory client type.
pub type InMemoryClient = sc_service::client::Client<
    InMemoryBackend,
    sc_service::LocalCallExecutor<
        Block,
        sc_fast_sync_backend::Backend<Block>,
        NativeElseWasmExecutor<BitcoinExecutorDispatch>,
    >,
    Block,
    RuntimeApi,
>;
pub type InMemoryBackend = sc_fast_sync_backend::Backend<Block>;

pub struct BitcoinExecutorDispatch;

impl NativeExecutionDispatch for BitcoinExecutorDispatch {
    type ExtendHostFunctions = ();

    fn dispatch(method: &str, data: &[u8]) -> Option<Vec<u8>> {
        subcoin_runtime::api::dispatch(method, data)
    }

    fn native_version() -> sc_executor::NativeVersion {
        subcoin_runtime::native_version()
    }
}

/// Builds a future that processes system RPC requests.
///
/// This is adapted from the upstream `build_system_rpc_future`.
async fn build_system_rpc_future<Block, Client>(
    role: Role,
    client: Arc<Client>,
    mut rpc_rx: TracingUnboundedReceiver<sc_rpc::system::Request<Block>>,
    should_have_peers: bool,
) where
    Block: BlockT,
    Client: HeaderBackend<Block> + Send + Sync + 'static,
{
    // Current best block at initialization, to report to the RPC layer.
    let starting_block = client.info().best_number;

    loop {
        // Answer incoming RPC requests.
        let Some(req) = rpc_rx.next().await else {
            tracing::debug!(
                "RPC requests stream has terminated, shutting down the system RPC future."
            );
            return;
        };

        match req {
            sc_rpc::system::Request::Health(sender) => {
                // TODO: Proper is_major_syncing
                let _ = sender.send(sc_rpc::system::Health {
                    peers: 0,
                    is_syncing: false,
                    should_have_peers,
                });
            }
            sc_rpc::system::Request::LocalPeerId(sender) => {
                // TODO: Proper local peer id.
                let _ = sender.send(PeerId::random().to_base58());
            }
            sc_rpc::system::Request::LocalListenAddresses(sender) => {
                // TODO: Proper addresses
                let _ = sender.send(Vec::new());
            }
            sc_rpc::system::Request::Peers(sender) => {
                // TODO: Proper peers
                let _ = sender.send(Vec::new());
            }
            sc_rpc::system::Request::NetworkState(sender) => {
                // TODO: Proper network state.
                let _ = sender.send(serde_json::Value::Null);
            }
            sc_rpc::system::Request::NetworkAddReservedPeer(_peer_addr, _sender) => {
                unreachable!("NetworkAddReservedPeer");
            }
            sc_rpc::system::Request::NetworkRemoveReservedPeer(_peer_id, _sender) => {
                unreachable!("NetworkRemoveReservedPeer");
            }
            sc_rpc::system::Request::NetworkReservedPeers(sender) => {
                let _ = sender.send(Vec::new());
            }
            sc_rpc::system::Request::NodeRoles(sender) => {
                use sc_rpc::system::NodeRole;

                let node_role = match role {
                    Role::Authority { .. } => NodeRole::Authority,
                    Role::Full => NodeRole::Full,
                };

                let _ = sender.send(vec![node_role]);
            }
            sc_rpc::system::Request::SyncState(sender) => {
                use sc_rpc::system::SyncState;

                let best_number = client.info().best_number;

                // TODO: Proper highest_block
                let _ = sender.send(SyncState {
                    starting_block,
                    current_block: best_number,
                    highest_block: best_number,
                });
            }
        }
    }
}

pub struct CoinStorageKey;

impl subcoin_primitives::CoinStorageKey for CoinStorageKey {
    fn storage_key(&self, txid: bitcoin::Txid, vout: u32) -> Vec<u8> {
        pallet_bitcoin::coin_storage_key::<subcoin_runtime::Runtime>(txid, vout)
    }

    fn storage_prefix(&self) -> [u8; 32] {
        pallet_bitcoin::coin_storage_prefix::<subcoin_runtime::Runtime>()
    }
}

/// Subcoin node components.
pub struct NodeComponents {
    /// Client.
    pub client: Arc<FullClient>,
    /// Backend.
    pub backend: Arc<FullBackend>,
    /// Executor
    pub executor: NativeElseWasmExecutor<BitcoinExecutorDispatch>,
    /// Task manager.
    pub task_manager: TaskManager,
    /// Block processor used in the block import pipeline.
    pub block_executor: Box<dyn BlockExecutor<Block>>,
    /// TODO: useless, remove later?
    pub system_rpc_tx: TracingUnboundedSender<sc_rpc::system::Request<Block>>,
}

/// Subcoin node configuration.
pub struct SubcoinConfiguration<'a> {
    pub network: bitcoin::Network,
    pub config: &'a Configuration,
    pub block_execution_strategy: BlockExecutionStrategy,
    pub no_hardware_benchmarks: bool,
    pub storage_monitor: sc_storage_monitor::StorageMonitorParams,
}

impl<'a> Deref for SubcoinConfiguration<'a> {
    type Target = Configuration;

    fn deref(&self) -> &Self::Target {
        self.config
    }
}

fn initialize_genesis_block_hash_mapping<Block: BlockT, Client: HeaderBackend<Block> + AuxStore>(
    client: &Client,
    bitcoin_network: bitcoin::Network,
) {
    // Initialize the genesis block hash mapping.
    let substrate_genesis_hash: <Block as BlockT>::Hash = client.info().genesis_hash;
    let bitcoin_genesis_hash = bitcoin::constants::genesis_block(bitcoin_network).block_hash();
    client
        .insert_aux(
            &[(
                bitcoin_genesis_hash.to_byte_array().as_slice(),
                substrate_genesis_hash.encode().as_slice(),
            )],
            [],
        )
        .expect("Failed to store genesis block hash mapping");
}

/// Creates a new subcoin node.
pub fn new_node(config: SubcoinConfiguration) -> Result<NodeComponents, ServiceError> {
    let SubcoinConfiguration {
        network: bitcoin_network,
        config,
        block_execution_strategy,
        no_hardware_benchmarks,
        storage_monitor,
    } = config;

    let telemetry = config
        .telemetry_endpoints
        .clone()
        .filter(|x| !x.is_empty())
        .map(|endpoints| -> Result<_, sc_telemetry::Error> {
            let worker = TelemetryWorker::new(16)?;
            let telemetry = worker.handle().new_telemetry(endpoints);
            Ok((worker, telemetry))
        })
        .transpose()?;

    // TODO: maintain the native executor on our own since it's deprecated upstream
    let executor = sc_service::new_native_or_wasm_executor(config);

    let backend = sc_service::new_db_backend(config.db_config())?;

    let genesis_block_builder = GenesisBlockBuilder::<_, _, _, TransactionAdapter>::new(
        bitcoin_network,
        config.chain_spec.as_storage_builder(),
        !config.no_genesis(),
        backend.clone(),
        executor.clone(),
    )?;

    let (client, backend, _keystore_container, task_manager) =
        sc_service::new_full_parts_with_genesis_builder::<Block, RuntimeApi, _, _>(
            config,
            telemetry.as_ref().map(|(_, telemetry)| telemetry.handle()),
            executor.clone(),
            backend,
            genesis_block_builder,
            false,
        )?;

    // Initialize the genesis block hash mapping.
    initialize_genesis_block_hash_mapping(&client, bitcoin_network);

    let client = Arc::new(client);

    let should_create_in_memory_client = block_execution_strategy.in_memory_backend_used();
    let block_executor = new_block_executor(
        client.clone(),
        block_execution_strategy,
        if should_create_in_memory_client {
            Some(new_in_memory_client(
                client.clone(),
                backend.clone(),
                executor.clone(),
                bitcoin_network,
                task_manager.spawn_handle(),
                config,
            )?)
        } else {
            None
        },
    );

    let mut telemetry = telemetry.map(|(worker, telemetry)| {
        task_manager
            .spawn_handle()
            .spawn("telemetry", None, worker.run());
        telemetry
    });

    let has_bootnodes = false;

    let spawner = task_manager.spawn_handle();
    let (system_rpc_tx, system_rpc_rx) = tracing_unbounded("mpsc_system_rpc", 10_000);
    spawner.spawn(
        "system-rpc-handler",
        Some("networking"),
        build_system_rpc_future(
            config.role.clone(),
            client.clone(),
            system_rpc_rx,
            has_bootnodes,
        ),
    );

    let database_path = config.database.path().map(Path::to_path_buf);
    let maybe_hwbench = (!no_hardware_benchmarks)
        .then_some(database_path.as_ref().map(|db_path| {
            let _ = std::fs::create_dir_all(db_path);
            sc_sysinfo::gather_hwbench(Some(db_path))
        }))
        .flatten();

    if let Some(hwbench) = maybe_hwbench {
        sc_sysinfo::print_hwbench(&hwbench);
        match SUBSTRATE_REFERENCE_HARDWARE.check_hardware(&hwbench) {
            Err(err) if config.role.is_authority() => {
                tracing::warn!(
					"⚠️  The hardware does not meet the minimal requirements {err} for role 'Authority'.",
				);
            }
            _ => {}
        }

        if let Some(ref mut telemetry) = telemetry {
            let telemetry_handle = telemetry.handle();
            task_manager.spawn_handle().spawn(
                "telemetry_hwbench",
                None,
                sc_sysinfo::initialize_hwbench_telemetry(telemetry_handle, hwbench),
            );
        }
    }

    if let Some(database_path) = database_path {
        sc_storage_monitor::StorageMonitorService::try_spawn(
            storage_monitor,
            database_path,
            &task_manager.spawn_essential_handle(),
        )
        .map_err(|e| ServiceError::Application(e.into()))?;
    }

    Ok(NodeComponents {
        client,
        backend,
        executor,
        task_manager,
        block_executor,
        system_rpc_tx,
    })
}

/// Runs the Substrate networking.
pub fn start_substrate_network<N>(
    config: Configuration,
    client: Arc<FullClient>,
    backend: Arc<FullBackend>,
    task_manager: &mut TaskManager,
    keystore: KeystorePtr,
    mut telemetry: Option<Telemetry>,
) -> Result<(), ServiceError>
where
    N: sc_network::NetworkBackend<Block, <Block as BlockT>::Hash>,
{
    let net_config =
        sc_network::config::FullNetworkConfiguration::<Block, <Block as BlockT>::Hash, N>::new(
            &config.network,
        );
    let metrics = N::register_notification_metrics(config.prometheus_registry());

    let transaction_pool = sc_transaction_pool::BasicPool::new_full(
        config.transaction_pool.clone(),
        config.role.is_authority().into(),
        config.prometheus_registry(),
        task_manager.spawn_essential_handle(),
        client.clone(),
    );

    let import_queue = BasicQueue::new(
        VerifyNothing,
        Box::new(client.clone()),
        None,
        &task_manager.spawn_essential_handle(),
        None,
    );

    let (network, system_rpc_tx, tx_handler_controller, network_starter, sync_service) =
        sc_service::build_network(sc_service::BuildNetworkParams {
            config: &config,
            net_config,
            client: client.clone(),
            transaction_pool: transaction_pool.clone(),
            spawn_handle: task_manager.spawn_handle(),
            import_queue,
            block_announce_validator_builder: None,
            warp_sync_params: None,
            block_relay: None,
            metrics,
        })?;

    let rpc_extensions_builder = {
        // RPCs are started in `new_node`.
        Box::new(move |_deny_unsafe, _| Ok(jsonrpsee::RpcModule::<()>::new(())))
    };

    let _rpc_handlers = sc_service::spawn_tasks(sc_service::SpawnTasksParams {
        network: Arc::new(network.clone()),
        client: client.clone(),
        keystore,
        task_manager,
        transaction_pool: transaction_pool.clone(),
        rpc_builder: rpc_extensions_builder,
        backend,
        system_rpc_tx,
        tx_handler_controller,
        sync_service: sync_service.clone(),
        config,
        telemetry: telemetry.as_mut(),
    })?;

    network_starter.start_network();

    Ok(())
}

/// Creates a future to finalize blocks with enough confirmations.
///
/// The future needs to be spawned in the background.
pub async fn finalize_confirmed_blocks<Block, Client, Backend>(
    client: Arc<Client>,
    spawn_handle: impl SpawnNamed,
    confirmation_depth: u32,
    major_sync_confirmation_depth: u32,
    is_major_syncing: Arc<AtomicBool>,
) where
    Block: BlockT + 'static,
    Client: HeaderBackend<Block> + Finalizer<Block, Backend> + BlockchainEvents<Block> + 'static,
    Backend: sc_client_api::backend::Backend<Block> + 'static,
{
    let mut block_import_stream = client.import_notification_stream();

    while let Some(notification) = block_import_stream.next().await {
        let block_number = client
            .number(notification.hash)
            .ok()
            .flatten()
            .expect("Imported Block must be available; qed");

        let Some(confirmed_block_number) = block_number.checked_sub(&confirmation_depth.into())
        else {
            continue;
        };

        let finalized_number = client.info().finalized_number;

        if confirmed_block_number <= finalized_number {
            continue;
        }

        if is_major_syncing.load(Ordering::SeqCst) {
            // During major sync, finalize every 10th block to avoid race conditions:
            // >Safety violation: attempted to revert finalized block...
            if confirmed_block_number < finalized_number + major_sync_confirmation_depth.into() {
                continue;
            }
        }

        let block_to_finalize = client
            .hash(confirmed_block_number)
            .ok()
            .flatten()
            .expect("Confirmed block must be available; qed");

        let client = client.clone();
        let is_major_syncing = is_major_syncing.clone();

        spawn_handle.spawn(
            "finalize-block",
            None,
            Box::pin(async move {
                match client.finalize_block(block_to_finalize, None, true) {
                    Ok(()) => {
                        if !is_major_syncing.load(Ordering::Relaxed) {
                            tracing::info!("✅ Successfully finalized block: {block_to_finalize}");
                        }
                    }
                    Err(err) => {
                        tracing::warn!(
                            ?err,
                            ?finalized_number,
                            "Failed to finalize block #{confirmed_block_number},{block_to_finalize}",
                        );
                    }
                }
            }),
        );
    }
}

type PartialComponents = sc_service::PartialComponents<
    FullClient,
    FullBackend,
    FullSelectChain,
    sc_consensus::DefaultImportQueue<Block>,
    sc_transaction_pool::FullPool<Block, FullClient>,
    Option<Telemetry>,
>;

/// Creates a partial node, for the chain ops commands.
pub fn new_partial(config: &Configuration) -> Result<PartialComponents, ServiceError> {
    let telemetry = config
        .telemetry_endpoints
        .clone()
        .filter(|x| !x.is_empty())
        .map(|endpoints| -> Result<_, sc_telemetry::Error> {
            let worker = TelemetryWorker::new(16)?;
            let telemetry = worker.handle().new_telemetry(endpoints);
            Ok((worker, telemetry))
        })
        .transpose()?;

    let executor = sc_service::new_native_or_wasm_executor(config);

    let (client, backend, keystore_container, task_manager) =
        sc_service::new_full_parts::<Block, RuntimeApi, _>(
            config,
            telemetry.as_ref().map(|(_, telemetry)| telemetry.handle()),
            executor,
        )?;
    let client = Arc::new(client);

    let telemetry = telemetry.map(|(worker, telemetry)| {
        task_manager
            .spawn_handle()
            .spawn("telemetry", None, worker.run());
        telemetry
    });

    let select_chain = sc_consensus::LongestChain::new(backend.clone());

    let transaction_pool = sc_transaction_pool::BasicPool::new_full(
        config.transaction_pool.clone(),
        config.role.is_authority().into(),
        config.prometheus_registry(),
        task_manager.spawn_essential_handle(),
        client.clone(),
    );

    let import_queue = BasicQueue::new(
        VerifyNothing,
        Box::new(client.clone()),
        None,
        &task_manager.spawn_essential_handle(),
        None,
    );

    Ok(sc_service::PartialComponents {
        client,
        backend,
        task_manager,
        import_queue,
        keystore_container,
        select_chain,
        transaction_pool,
        other: (telemetry),
    })
}

pub struct VerifyNothing;

#[async_trait::async_trait]
impl<Block: BlockT> Verifier<Block> for VerifyNothing {
    async fn verify(
        &mut self,
        block: BlockImportParams<Block>,
    ) -> Result<BlockImportParams<Block>, String> {
        Ok(BlockImportParams::new(block.origin, block.header))
    }
}
