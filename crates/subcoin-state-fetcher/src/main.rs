mod cli;

use clap::Parser;
use sc_cli::{NetworkParams as SubstrateNetworkParams, SubstrateCli};
use sc_consensus::import_queue::BasicQueue;
use sc_consensus_nakamoto::SubstrateImportQueueVerifier;
use sc_network::config::NetworkBackendType;
use sc_service::config::{
    BlocksPruning, DatabaseSource, ExecutorConfiguration, KeystoreConfig, NetworkConfiguration,
    OffchainWorkerConfig, RpcBatchRequestConfig, RpcConfiguration, WasmExecutionMethod,
    WasmtimeInstantiationStrategy,
};
use sc_service::{BasePath, Configuration, Role, TaskManager};
use sp_runtime::traits::Block as BlockT;
use std::path::PathBuf;
use std::sync::Arc;
use subcoin_runtime::interface::OpaqueBlock as Block;
use subcoin_runtime::RuntimeApi;
use subcoin_service::{FullClient, GenesisBlockBuilder, TransactionAdapter};

/// Subcoin UTXO Set State Download Tool
#[derive(Debug, Parser)]
#[clap(version = "0.1.0")]
pub struct Cli {
    /// Specify custom base path.
    #[arg(long, short = 'd', value_name = "PATH")]
    pub base_path: Option<PathBuf>,

    #[allow(missing_docs)]
    #[clap(flatten)]
    pub network_params: SubstrateNetworkParams,
}

fn main() -> sc_cli::Result<()> {
    let Cli {
        base_path,
        network_params,
    } = Cli::parse();

    let base_path = base_path
        .map(BasePath::from)
        .unwrap_or_else(|| BasePath::from_project("", "", "subcoin_state_sync_node"));

    let bitcoin_network = bitcoin::Network::Bitcoin;

    let tokio_runtime = sc_cli::build_runtime()?;

    // `capture` needs to be called in a tokio context.
    // Also capture them as early as possible.
    let signals = tokio_runtime.block_on(async { sc_cli::Signals::capture() })?;

    let chain_spec = cli::SubstrateCli.load_spec("bitcoin-mainnet")?;
    let config_dir = base_path.config_dir(chain_spec.id());
    let net_config_dir = config_dir.join("network");
    let client_id = cli::SubstrateCli::client_id();

    let node_name = sc_cli::generate_node_name();
    let node_key = network_params
        .node_key_params
        .node_key(&net_config_dir, Role::Full, false)?;
    let network_config = network_params.network_config(
        &chain_spec,
        false,
        false,
        Some(net_config_dir),
        &client_id,
        &node_name,
        node_key,
        30333,
    );

    let tokio_handle = tokio_runtime.handle().clone();
    let config = create_configuration(
        tokio_handle.clone(),
        bitcoin_network,
        base_path,
        network_config,
    );

    let runner = sc_cli::Runner::<cli::SubstrateCli>::new(config, tokio_runtime, signals)?;

    runner
        .run_node_until_exit(|config| async move { new_state_sync_client(bitcoin_network, config) })
        .map_err(Into::into)
}

fn create_configuration(
    tokio_handle: tokio::runtime::Handle,
    bitcoin_network: bitcoin::Network,
    base_path: BasePath,
    network_config: NetworkConfiguration,
) -> Configuration {
    let root = PathBuf::from("/tmp");
    let chain_spec =
        subcoin_service::chain_spec::config(bitcoin_network).expect("Failed to create chain spec");
    Configuration {
        impl_name: "subcoin-state-sync-node".to_string(),
        impl_version: "0.1.0".to_string(),
        role: Role::Full,
        tokio_handle,
        transaction_pool: Default::default(),
        network: network_config,
        keystore: KeystoreConfig::InMemory,
        database: DatabaseSource::ParityDb {
            path: root.join("db"),
        },
        trie_cache_maximum_size: None,
        state_pruning: None,
        blocks_pruning: BlocksPruning::KeepAll,
        chain_spec: Box::new(chain_spec),
        executor: ExecutorConfiguration {
            wasm_method: WasmExecutionMethod::Compiled {
                instantiation_strategy: WasmtimeInstantiationStrategy::PoolingCopyOnWrite,
            },
            max_runtime_instances: 8,
            runtime_cache_size: 2,
            default_heap_pages: None,
        },
        rpc: RpcConfiguration {
            addr: None,
            max_connections: Default::default(),
            cors: None,
            methods: Default::default(),
            max_request_size: Default::default(),
            max_response_size: Default::default(),
            id_provider: Default::default(),
            max_subs_per_conn: Default::default(),
            port: 9944,
            message_buffer_capacity: Default::default(),
            batch_config: RpcBatchRequestConfig::Unlimited,
            rate_limit: None,
            rate_limit_whitelisted_ips: Default::default(),
            rate_limit_trust_proxy_headers: Default::default(),
        },
        prometheus_config: None,
        telemetry_endpoints: None,
        offchain_worker: OffchainWorkerConfig {
            enabled: true,
            indexing_enabled: false,
        },
        force_authoring: false,
        disable_grandpa: false,
        dev_key_seed: None,
        tracing_targets: None,
        tracing_receiver: Default::default(),
        announce_block: true,
        data_path: base_path.path().into(),
        base_path,
        wasm_runtime_overrides: None,
    }
}

fn new_state_sync_client(
    bitcoin_network: bitcoin::Network,
    mut config: Configuration,
) -> Result<TaskManager, sc_service::error::Error> {
    let executor = sc_service::new_native_or_wasm_executor(&config);

    let backend = sc_service::new_db_backend(config.db_config())?;

    let commit_genesis_state = true;

    let genesis_block_builder = GenesisBlockBuilder::<_, _, _, TransactionAdapter>::new(
        bitcoin_network,
        config.chain_spec.as_storage_builder(),
        commit_genesis_state,
        backend.clone(),
        executor.clone(),
    )?;

    let (client, _backend, _keystore_container, mut task_manager) =
        sc_service::new_full_parts_with_genesis_builder::<Block, RuntimeApi, _, _>(
            &config,
            None,
            executor.clone(),
            backend,
            genesis_block_builder,
            false,
        )?;

    // Initialize the genesis block hash mapping.
    subcoin_service::initialize_genesis_block_hash_mapping(&client, bitcoin_network);

    let client = Arc::new(client);

    let network_backend = NetworkBackendType::Libp2p;

    match network_backend {
        NetworkBackendType::Libp2p => start_substrate_network::<
            sc_network::NetworkWorker<Block, <Block as BlockT>::Hash>,
        >(
            &mut config, client, &mut task_manager, bitcoin_network
        )?,
        NetworkBackendType::Litep2p => {
            start_substrate_network::<sc_network::Litep2pNetworkBackend>(
                &mut config,
                client,
                &mut task_manager,
                bitcoin_network,
            )?;
        }
    }

    Ok(task_manager)
}

fn start_substrate_network<N>(
    config: &mut Configuration,
    client: Arc<FullClient>,
    task_manager: &mut sc_service::TaskManager,
    bitcoin_network: bitcoin::Network,
) -> Result<(), sc_service::error::Error>
where
    N: sc_network::NetworkBackend<Block, <Block as BlockT>::Hash>,
{
    let mut net_config = sc_network::config::FullNetworkConfiguration::<
        Block,
        <Block as BlockT>::Hash,
        N,
    >::new(&config.network, None);

    let transaction_pool = sc_transaction_pool::BasicPool::new_full(
        config.transaction_pool.clone(),
        false.into(),
        None,
        task_manager.spawn_essential_handle(),
        client.clone(),
    );

    let import_queue = BasicQueue::new(
        SubstrateImportQueueVerifier::new(client.clone(), bitcoin_network),
        Box::new(client.clone()),
        None,
        &task_manager.spawn_essential_handle(),
        None,
    );

    let syncing_strategy = sc_service::build_polkadot_syncing_strategy(
        config.protocol_id(),
        config.chain_spec.fork_id(),
        &mut net_config,
        None,
        client.clone(),
        &task_manager.spawn_handle(),
        None,
    )?;

    let metrics = N::register_notification_metrics(config.prometheus_registry());

    let (network, _system_rpc_tx, _tx_handler_controller, network_starter, sync_service) =
        sc_service::build_network(sc_service::BuildNetworkParams {
            config,
            net_config,
            client: client.clone(),
            transaction_pool: transaction_pool.clone(),
            spawn_handle: task_manager.spawn_handle(),
            import_queue,
            block_announce_validator_builder: None,
            syncing_strategy,
            block_relay: None,
            metrics,
        })?;

    let spawn_handle = task_manager.spawn_handle();

    spawn_handle.spawn(
        "substrate-informant",
        None,
        sc_informant::build(client.clone(), Arc::new(network), sync_service.clone()),
    );

    network_starter.start_network();

    Ok(())
}
