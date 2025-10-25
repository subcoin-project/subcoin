//! Subcoin service implementation. Specialized wrapper over substrate service.

#![allow(deprecated)]
#![allow(clippy::result_large_err)]

pub mod chain_spec;
mod finalizer;
mod genesis_block_builder;
pub mod network_request_handler;
mod transaction_adapter;
pub mod transaction_pool;

use bitcoin::hashes::Hash;
use futures::FutureExt;
use sc_client_api::{AuxStore, HeaderBackend};
use sc_consensus::import_queue::BasicQueue;
use sc_consensus_nakamoto::SubstrateImportQueueVerifier;
use sc_executor::NativeElseWasmExecutor;
use sc_network_sync::SyncingService;
use sc_service::config::PrometheusConfig;
use sc_service::error::Error as ServiceError;
use sc_service::{
    Configuration, KeystoreContainer, MetricsService, NativeExecutionDispatch, TaskManager,
};
use sc_telemetry::{Telemetry, TelemetryWorker};
use sc_utils::mpsc::TracingUnboundedSender;
use sp_core::Encode;
use sp_runtime::traits::Block as BlockT;
use std::ops::Deref;
use std::path::Path;
use std::sync::Arc;
use subcoin_runtime::RuntimeApi;
use subcoin_runtime::interface::OpaqueBlock as Block;

pub use finalizer::SubcoinFinalizer;
pub use genesis_block_builder::GenesisBlockBuilder;
pub use transaction_adapter::TransactionAdapter;

/// This is a specialization of the general Substrate ChainSpec type.
pub type ChainSpec = sc_service::GenericChainSpec;

/// Disk backend client type.
pub type FullClient =
    sc_service::TFullClient<Block, RuntimeApi, NativeElseWasmExecutor<SubcoinExecutorDispatch>>;
pub type FullBackend = sc_service::TFullBackend<Block>;
type FullSelectChain = sc_consensus::LongestChain<FullBackend, Block>;

/// Subcoin executor.
pub struct SubcoinExecutorDispatch;

impl NativeExecutionDispatch for SubcoinExecutorDispatch {
    type ExtendHostFunctions = ();

    fn dispatch(method: &str, data: &[u8]) -> Option<Vec<u8>> {
        subcoin_runtime::api::dispatch(method, data)
    }

    fn native_version() -> sc_executor::NativeVersion {
        subcoin_runtime::native_version()
    }
}

/// This struct implements the trait [`subcoin_primitives::CoinStorageKey`].
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
    pub executor: NativeElseWasmExecutor<SubcoinExecutorDispatch>,
    /// Task manager.
    pub task_manager: TaskManager,
    pub keystore_container: KeystoreContainer,
    pub telemetry: Option<Telemetry>,
}

/// Subcoin node configuration.
pub struct SubcoinConfiguration<'a> {
    pub network: bitcoin::Network,
    pub config: &'a Configuration,
    pub no_hardware_benchmarks: bool,
    pub storage_monitor: sc_storage_monitor::StorageMonitorParams,
}

impl<'a> SubcoinConfiguration<'a> {
    /// Creates a [`SubcoinConfiguration`] for test purposes.
    pub fn test_config(config: &'a Configuration) -> Self {
        Self {
            network: bitcoin::Network::Bitcoin,
            config,
            no_hardware_benchmarks: true,
            storage_monitor: Default::default(),
        }
    }
}

impl Deref for SubcoinConfiguration<'_> {
    type Target = Configuration;

    fn deref(&self) -> &Self::Target {
        self.config
    }
}

/// Insert the genesis block hash mapping into aux-db.
pub fn initialize_genesis_block_hash_mapping<
    Block: BlockT,
    Client: HeaderBackend<Block> + AuxStore,
>(
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
        no_hardware_benchmarks: _,
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

    let (client, backend, keystore_container, task_manager) =
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

    let telemetry = telemetry.map(|(worker, telemetry)| {
        task_manager
            .spawn_handle()
            .spawn("telemetry", None, worker.run());
        telemetry
    });

    let database_path = config.database.path().map(Path::to_path_buf);

    // TODO: frame_benchmarking_cli pulls in rocksdb due to its dep
    // cumulus-client-parachain-inherent.
    // let maybe_hwbench = (!no_hardware_benchmarks)
    // .then(|| {database_path.as_ref().map(|db_path| {
    // let _ = std::fs::create_dir_all(db_path);
    // sc_sysinfo::gather_hwbench(Some(db_path), &frame_benchmarking_cli::SUBSTRATE_REFERENCE_HARDWARE)
    // })})
    // .flatten();

    // if let Some(hwbench) = maybe_hwbench {
    // sc_sysinfo::print_hwbench(&hwbench);
    // match frame_benchmarking_cli::SUBSTRATE_REFERENCE_HARDWARE.check_hardware(&hwbench, config.role.is_authority()) {
    // Err(err) if config.role.is_authority() => {
    // tracing::warn!(
    // "⚠️  The hardware does not meet the minimal requirements {err} for role 'Authority'.",
    // );
    // }
    // _ => {}
    // }

    // if let Some(ref mut telemetry) = telemetry {
    // let telemetry_handle = telemetry.handle();
    // task_manager.spawn_handle().spawn(
    // "telemetry_hwbench",
    // None,
    // sc_sysinfo::initialize_hwbench_telemetry(telemetry_handle, hwbench),
    // );
    // }
    // }

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
        keystore_container,
        telemetry,
    })
}

type SubstrateNetworkingParts = (
    TracingUnboundedSender<sc_rpc::system::Request<Block>>,
    Arc<SyncingService<Block>>,
);

/// Runs the Substrate networking.
pub fn start_substrate_network<N>(
    config: &mut Configuration,
    client: Arc<FullClient>,
    _backend: Arc<FullBackend>,
    task_manager: &mut TaskManager,
    bitcoin_network: bitcoin::Network,
    mut telemetry: Option<Telemetry>,
) -> Result<SubstrateNetworkingParts, ServiceError>
where
    N: sc_network::NetworkBackend<Block, <Block as BlockT>::Hash>,
{
    let mut net_config = sc_network::config::FullNetworkConfiguration::<
        Block,
        <Block as BlockT>::Hash,
        N,
    >::new(&config.network, config.prometheus_registry().cloned());

    let network_request_protocol_config = {
        // Allow both outgoing and incoming requests.
        let (handler, protocol_config) = network_request_handler::NetworkRequestHandler::new::<N>(
            &config.protocol_id(),
            config.chain_spec.fork_id(),
            client.clone(),
            100,
        );
        task_manager.spawn_handle().spawn(
            "subcoin-network-request-handler",
            Some("networking"),
            handler.run(),
        );
        protocol_config
    };

    net_config.add_request_response_protocol(network_request_protocol_config);

    let metrics = N::register_notification_metrics(config.prometheus_registry());

    let transaction_pool = Arc::from(
        sc_transaction_pool::Builder::new(
            task_manager.spawn_essential_handle(),
            client.clone(),
            config.role.is_authority().into(),
        )
        .with_options(config.transaction_pool.clone())
        .with_prometheus(config.prometheus_registry())
        .build(),
    );

    let import_queue = BasicQueue::new(
        SubstrateImportQueueVerifier::new(client.clone(), bitcoin_network),
        Box::new(client.clone()),
        None,
        &task_manager.spawn_essential_handle(),
        None,
    );

    let (network, system_rpc_tx, _tx_handler_controller, sync_service) =
        sc_service::build_network(sc_service::BuildNetworkParams {
            config,
            net_config,
            client: client.clone(),
            transaction_pool: transaction_pool.clone(),
            spawn_handle: task_manager.spawn_handle(),
            import_queue,
            block_announce_validator_builder: None,
            warp_sync_config: None,
            block_relay: None,
            metrics,
        })?;

    // Custom `sc_service::spawn_tasks` with some needless tasks removed.
    let spawn_handle = task_manager.spawn_handle();

    let sysinfo = sc_sysinfo::gather_sysinfo();
    sc_sysinfo::print_sysinfo(&sysinfo);

    let telemetry = telemetry
        .as_mut()
        .map(|telemetry| {
            sc_service::init_telemetry(
                config.network.node_name.clone(),
                config.impl_name.clone(),
                config.impl_version.clone(),
                config.chain_spec.name().to_string(),
                config.role.is_authority(),
                network.clone(),
                client.clone(),
                telemetry,
                Some(sysinfo),
            )
        })
        .transpose()?;

    // Prometheus metrics.
    let metrics_service =
        if let Some(PrometheusConfig { port, registry }) = config.prometheus_config.clone() {
            // Set static metrics.
            let metrics = MetricsService::with_prometheus(
                telemetry,
                &registry,
                config.role,
                &config.network.node_name,
                &config.impl_version,
            )?;
            spawn_handle.spawn(
                "prometheus-endpoint",
                None,
                substrate_prometheus_endpoint::init_prometheus(port, registry).map(drop),
            );

            metrics
        } else {
            MetricsService::new(telemetry)
        };

    // Periodically updated metrics and telemetry updates.
    spawn_handle.spawn(
        "telemetry-periodic-send",
        None,
        metrics_service.run(
            client.clone(),
            transaction_pool.clone(),
            network.clone(),
            sync_service.clone(),
        ),
    );

    spawn_handle.spawn(
        "substrate-informant",
        None,
        sc_informant::build(client.clone(), Arc::new(network), sync_service.clone()),
    );

    Ok((system_rpc_tx, sync_service))
}

type PartialComponents = sc_service::PartialComponents<
    FullClient,
    FullBackend,
    FullSelectChain,
    sc_consensus::DefaultImportQueue<Block>,
    sc_transaction_pool::TransactionPoolHandle<Block, FullClient>,
    Option<Telemetry>,
>;

/// Creates a partial node, for the chain ops commands.
pub fn new_partial(
    config: &Configuration,
    bitcoin_network: bitcoin::Network,
) -> Result<PartialComponents, ServiceError> {
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

    let transaction_pool = Arc::from(
        sc_transaction_pool::Builder::new(
            task_manager.spawn_essential_handle(),
            client.clone(),
            config.role.is_authority().into(),
        )
        .with_options(config.transaction_pool.clone())
        .with_prometheus(config.prometheus_registry())
        .build(),
    );

    let import_queue = BasicQueue::new(
        SubstrateImportQueueVerifier::new(client.clone(), bitcoin_network),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NodeComponents, SubcoinConfiguration, new_node};
    use subcoin_primitives::extract_bitcoin_block_hash;
    use tokio::runtime::Handle;

    #[tokio::test]
    async fn test_bitcoin_block_hash_in_substrate_header() {
        let runtime_handle = Handle::current();
        let config = subcoin_test_service::test_configuration(runtime_handle);
        let NodeComponents { client, .. } =
            new_node(SubcoinConfiguration::test_config(&config)).expect("Failed to create node");

        let substrate_genesis_header = client.header(client.info().genesis_hash).unwrap().unwrap();
        let bitcoin_genesis_header =
            bitcoin::constants::genesis_block(bitcoin::Network::Bitcoin).header;

        let bitcoin_genesis_block_hash = bitcoin_genesis_header.block_hash();

        assert_eq!(
            extract_bitcoin_block_hash::<subcoin_runtime::interface::OpaqueBlock>(
                &substrate_genesis_header
            )
            .unwrap(),
            bitcoin_genesis_block_hash,
        );
    }
}
