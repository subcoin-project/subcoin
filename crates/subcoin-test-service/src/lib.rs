#![allow(clippy::result_large_err)]

use bitcoin::Block;
use bitcoin::consensus::Decodable;
use bitcoin::hex::FromHex;
use sc_client_api::HeaderBackend;
use sc_consensus_nakamoto::{
    BitcoinBlockImport, BitcoinBlockImporter, BlockVerification, ImportConfig, ImportStatus,
    ScriptEngine,
};
use sc_network::config::{MultiaddrWithPeerId, NodeKeyConfig, Secret, TransportConfig};
use sc_network::{Multiaddr, NetworkBackend, Roles};
use sc_network_common::sync::SyncMode;
use sc_network_sync::engine::SyncingEngine;
use sc_network_sync::service::network::NetworkServiceProvider;
use sc_service::config::{
    BlocksPruning, DatabaseSource, ExecutorConfiguration, KeystoreConfig, NetworkConfiguration,
    OffchainWorkerConfig, PruningMode, RpcBatchRequestConfig, RpcConfiguration,
    WasmExecutionMethod, WasmtimeInstantiationStrategy,
};
use sc_service::error::Error as ServiceError;
use sc_service::{BasePath, ChainSpec, Configuration, ImportQueue, Role, TaskManager};
use sp_consensus::BlockOrigin;
use sp_keyring::sr25519::Keyring as Sr25519Keyring;
use sp_runtime::traits::Block as BlockT;
use std::sync::Arc;
use subcoin_service::{FullClient, NodeComponents, SubcoinConfiguration};

fn decode_raw_block(hex_str: &str) -> Block {
    let data = Vec::<u8>::from_hex(hex_str).expect("Failed to convert hex str");
    Block::consensus_decode(&mut data.as_slice()).expect("Failed to convert hex data to Block")
}

pub fn block_data() -> Vec<Block> {
    // genesis block
    let block0 = decode_raw_block(
        "0100000000000000000000000000000000000000000000000000000000000000000000003ba3edfd7a7b12b27ac72c3e67768f617fc81bc3888a51323a9fb8aa4b1e5e4a29ab5f49ffff001d1dac2b7c0101000000010000000000000000000000000000000000000000000000000000000000000000ffffffff4d04ffff001d0104455468652054696d65732030332f4a616e2f32303039204368616e63656c6c6f72206f6e206272696e6b206f66207365636f6e64206261696c6f757420666f722062616e6b73ffffffff0100f2052a01000000434104678afdb0fe5548271967f1a67130b7105cd6a828e03909a67962e0ea1f61deb649f6bc3f4cef38c4f35504e51ec112de5c384df7ba0b8d578a4c702b6bf11d5fac00000000",
    );
    // https://webbtc.com/block/00000000839a8e6886ab5951d76f411475428afc90947ee320161bbf18eb6048
    // height 1
    let block1 = decode_raw_block(
        "010000006fe28c0ab6f1b372c1a6a246ae63f74f931e8365e15a089c68d6190000000000982051fd1e4ba744bbbe680e1fee14677ba1a3c3540bf7b1cdb606e857233e0e61bc6649ffff001d01e362990101000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0704ffff001d0104ffffffff0100f2052a0100000043410496b538e853519c726a2c91e61ec11600ae1390813a627c66fb8be7947be63c52da7589379515d4e0a604f8141781e62294721166bf621e73a82cbf2342c858eeac00000000",
    );
    // https://webbtc.com/block/000000006a625f06636b8bb6ac7b960a8d03705d1ace08b1a19da3fdcc99ddbd.hex
    // height 2
    let block2 = decode_raw_block(
        "010000004860eb18bf1b1620e37e9490fc8a427514416fd75159ab86688e9a8300000000d5fdcc541e25de1c7a5addedf24858b8bb665c9f36ef744ee42c316022c90f9bb0bc6649ffff001d08d2bd610101000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0704ffff001d010bffffffff0100f2052a010000004341047211a824f55b505228e4c3d5194c1fcfaa15a456abdf37f9b9d97a4040afc073dee6c89064984f03385237d92167c13e236446b417ab79a0fcae412ae3316b77ac00000000",
    );
    // https://webbtc.com/block/0000000082b5015589a3fdf2d4baff403e6f0be035a5d9742c1cae6295464449.hex
    // height 3
    let block3 = decode_raw_block(
        "01000000bddd99ccfda39da1b108ce1a5d70038d0a967bacb68b6b63065f626a0000000044f672226090d85db9a9f2fbfe5f0f9609b387af7be5b7fbb7a1767c831c9e995dbe6649ffff001d05e0ed6d0101000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0704ffff001d010effffffff0100f2052a0100000043410494b9d3e76c5b1629ecf97fff95d7a4bbdac87cc26099ada28066c6ff1eb9191223cd897194a08d0c2726c5747f1db49e8cf90e75dc3e3550ae9b30086f3cd5aaac00000000",
    );
    // height 4
    let block4 = decode_raw_block(
        "010000004944469562ae1c2c74d9a535e00b6f3e40ffbad4f2fda3895501b582000000007a06ea98cd40ba2e3288262b28638cec5337c1456aaf5eedc8e9e5a20f062bdf8cc16649ffff001d2bfee0a90101000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0704ffff001d011affffffff0100f2052a01000000434104184f32b212815c6e522e66686324030ff7e5bf08efb21f8b00614fb7690e19131dd31304c54f37baa40db231c918106bb9fd43373e37ae31a0befc6ecaefb867ac00000000",
    );
    // height 5
    let block5 = decode_raw_block(
        "0100000085144a84488ea88d221c8bd6c059da090e88f8a2c99690ee55dbba4e00000000e11c48fecdd9e72510ca84f023370c9a38bf91ac5cae88019bee94d24528526344c36649ffff001d1d03e4770101000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0704ffff001d0120ffffffff0100f2052a0100000043410456579536d150fbce94ee62b47db2ca43af0a730a0467ba55c79e2a7ec9ce4ad297e35cdbb8e42a4643a60eef7c9abee2f5822f86b1da242d9c2301c431facfd8ac00000000",
    );
    // height 6
    let block6 = decode_raw_block(
        "01000000fc33f596f822a0a1951ffdbf2a897b095636ad871707bf5d3162729b00000000379dfb96a5ea8c81700ea4ac6b97ae9a9312b2d4301a29580e924ee6761a2520adc46649ffff001d189c4c970101000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0704ffff001d0123ffffffff0100f2052a0100000043410408ce279174b34c077c7b2043e3f3d45a588b85ef4ca466740f848ead7fb498f0a795c982552fdfa41616a7c0333a269d62108588e260fd5a48ac8e4dbf49e2bcac00000000",
    );
    // height 7
    let block7 = decode_raw_block(
        "010000008d778fdc15a2d3fb76b7122a3b5582bea4f21f5a0c693537e7a03130000000003f674005103b42f984169c7d008370967e91920a6a5d64fd51282f75bc73a68af1c66649ffff001d39a59c860101000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0704ffff001d012bffffffff0100f2052a01000000434104a59e64c774923d003fae7491b2a7f75d6b7aa3f35606a8ff1cf06cd3317d16a41aa16928b1df1f631f31f28c7da35d4edad3603adb2338c4d4dd268f31530555ac00000000",
    );
    // height 8
    let block8 = decode_raw_block(
        "010000004494c8cf4154bdcc0720cd4a59d9c9b285e4b146d45f061d2b6c967100000000e3855ed886605b6d4a99d5fa2ef2e9b0b164e63df3c4136bebf2d0dac0f1f7a667c86649ffff001d1c4b56660101000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0704ffff001d012cffffffff0100f2052a01000000434104cc8d85f5e7933cb18f13b97d165e1189c1fb3e9c98b0dd5446b2a1989883ff9e740a8a75da99cc59a21016caf7a7afd3e4e9e7952983e18d1ff70529d62e0ba1ac00000000",
    );
    // height 9
    let block9 = decode_raw_block(
        "01000000c60ddef1b7618ca2348a46e868afc26e3efc68226c78aa47f8488c4000000000c997a5e56e104102fa209c6a852dd90660a20b2d9c352423edce25857fcd37047fca6649ffff001d28404f530101000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0704ffff001d0134ffffffff0100f2052a0100000043410411db93e1dcdb8a016b49840f8c53bc1eb68a382e97b1482ecad7b148a6909a5cb2e0eaddfb84ccf9744464f82e160bfa9b8b64f9d4c03f999b8643f656b412a3ac00000000",
    );
    // height 10
    let block10 = decode_raw_block(
        "010000000508085c47cc849eb80ea905cc7800a3be674ffc57263cf210c59d8d00000000112ba175a1e04b14ba9e7ea5f76ab640affeef5ec98173ac9799a852fa39add320cd6649ffff001d1e2de5650101000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0704ffff001d0136ffffffff0100f2052a01000000434104fcc2888ca91cf0103d8c5797c256bf976e81f280205d002d85b9b622ed1a6f820866c7b5fe12285cfa78c035355d752fc94a398b67597dc4fbb5b386816425ddac00000000",
    );
    vec![
        block0, block1, block2, block3, block4, block5, block6, block7, block8, block9, block10,
    ]
}

pub fn full_test_configuration(
    tokio_handle: tokio::runtime::Handle,
    chain_spec: Box<dyn ChainSpec>,
) -> Configuration {
    let tmp = tempfile::tempdir().unwrap();
    let base_path = BasePath::new(tmp.path());
    let root = base_path.path().to_path_buf();

    let network_config = NetworkConfiguration::new(
        Sr25519Keyring::Alice.to_seed(),
        "network/test/0.1",
        Default::default(),
        None,
    );

    Configuration {
        impl_name: "subcoin-test-node".to_string(),
        impl_version: "1.0".to_string(),
        role: Role::Full,
        tokio_handle,
        transaction_pool: Default::default(),
        network: network_config,
        keystore: KeystoreConfig::InMemory,
        database: DatabaseSource::ParityDb {
            path: root.join("db"),
        },
        trie_cache_maximum_size: Some(64 * 1024 * 1024),
        state_pruning: Some(PruningMode::ArchiveAll),
        blocks_pruning: BlocksPruning::KeepAll,
        chain_spec,
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
            request_logger_limit: 1024,
        },
        prometheus_config: None,
        telemetry_endpoints: None,
        offchain_worker: OffchainWorkerConfig {
            enabled: true,
            indexing_enabled: false,
        },
        force_authoring: false,
        disable_grandpa: false,
        dev_key_seed: Some(Sr25519Keyring::Alice.to_seed()),
        tracing_targets: None,
        tracing_receiver: Default::default(),
        announce_block: true,
        data_path: base_path.path().into(),
        base_path,
        wasm_runtime_overrides: None,
        warm_up_trie_cache: None,
    }
}

pub fn test_configuration(tokio_handle: tokio::runtime::Handle) -> Configuration {
    let spec = subcoin_service::chain_spec::config(bitcoin::Network::Bitcoin)
        .expect("Failed to create chain spec");

    full_test_configuration(tokio_handle, Box::new(spec))
}

pub fn new_test_node(tokio_handle: tokio::runtime::Handle) -> Result<NodeComponents, ServiceError> {
    let config = test_configuration(tokio_handle);
    subcoin_service::new_node(SubcoinConfiguration {
        network: bitcoin::Network::Bitcoin,
        config: &config,
        no_hardware_benchmarks: true,
        storage_monitor: Default::default(),
    })
}

pub async fn new_test_node_and_produce_blocks(
    config: &Configuration,
    up_to: u32,
) -> Arc<FullClient> {
    let NodeComponents { client, .. } =
        subcoin_service::new_node(SubcoinConfiguration::test_config(config))
            .expect("Failed to create node");

    let mut bitcoin_block_import =
        BitcoinBlockImporter::<_, _, _, _, subcoin_service::TransactionAdapter>::new(
            client.clone(),
            client.clone(),
            ImportConfig {
                network: bitcoin::Network::Bitcoin,
                block_verification: BlockVerification::None,
                execute_block: true,
                script_engine: ScriptEngine::Core,
            },
            Arc::new(subcoin_service::CoinStorageKey),
            None,
        );

    let test_blocks = block_data();

    if up_to as usize >= test_blocks.len() {
        panic!(
            "Cannot produce {up_to} blocks (maximum: {}) in test env",
            test_blocks.len() - 1
        );
    }

    for block_number in 1..=up_to {
        let block = test_blocks[block_number as usize].clone();
        let import_status = bitcoin_block_import
            .import_block(block, BlockOrigin::Own)
            .await
            .unwrap();
        assert!(matches!(import_status, ImportStatus::Imported { .. }));
    }

    client
}

/// Configuration for a test node with networking enabled.
#[derive(Default)]
pub struct TestNetworkConfig {
    /// The port to listen on for P2P connections.
    pub listen_port: u16,
    /// Optional bootnodes to connect to.
    pub boot_nodes: Vec<MultiaddrWithPeerId>,
    /// The node key seed (for deterministic peer IDs).
    pub node_key_seed: [u8; 32],
}


/// Create a test configuration with networking enabled.
///
/// This allows creating nodes that can connect to each other via Substrate P2P.
pub fn test_configuration_with_networking(
    tokio_handle: tokio::runtime::Handle,
    network_config: TestNetworkConfig,
) -> Configuration {
    let spec = subcoin_service::chain_spec::config(bitcoin::Network::Bitcoin)
        .expect("Failed to create chain spec");

    let tmp = tempfile::tempdir().unwrap();
    let base_path = BasePath::new(tmp.path());
    let root = base_path.path().to_path_buf();

    // Configure listen address
    let listen_addr: Multiaddr = format!("/ip4/127.0.0.1/tcp/{}", network_config.listen_port)
        .parse()
        .expect("Valid multiaddr");

    let mut sc_network_config = NetworkConfiguration::new(
        Sr25519Keyring::Alice.to_seed(),
        "network/test/0.1",
        Default::default(),
        None,
    );

    // Set listen addresses
    sc_network_config.listen_addresses = vec![listen_addr];
    sc_network_config.boot_nodes = network_config.boot_nodes;
    // Use Normal transport for TCP connections, disable mDNS for test isolation
    sc_network_config.transport = TransportConfig::Normal {
        enable_mdns: false,
        allow_private_ip: true,
    };

    // Use deterministic node key for predictable peer IDs
    // Generate a secret key from the seed
    let secret_key =
        sc_network::config::ed25519::SecretKey::try_from_bytes(network_config.node_key_seed)
            .expect("Valid secret key");
    sc_network_config.node_key = NodeKeyConfig::Ed25519(Secret::Input(secret_key));

    Configuration {
        impl_name: "subcoin-test-node".to_string(),
        impl_version: "1.0".to_string(),
        role: Role::Full,
        tokio_handle,
        transaction_pool: Default::default(),
        network: sc_network_config,
        keystore: KeystoreConfig::InMemory,
        database: DatabaseSource::ParityDb {
            path: root.join("db"),
        },
        trie_cache_maximum_size: Some(64 * 1024 * 1024),
        state_pruning: Some(PruningMode::ArchiveAll),
        blocks_pruning: BlocksPruning::KeepAll,
        chain_spec: Box::new(spec),
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
            request_logger_limit: 1024,
        },
        prometheus_config: None,
        telemetry_endpoints: None,
        offchain_worker: OffchainWorkerConfig {
            enabled: true,
            indexing_enabled: false,
        },
        force_authoring: false,
        disable_grandpa: false,
        dev_key_seed: Some(Sr25519Keyring::Alice.to_seed()),
        tracing_targets: None,
        tracing_receiver: Default::default(),
        announce_block: true,
        data_path: base_path.path().into(),
        base_path,
        wasm_runtime_overrides: None,
        warm_up_trie_cache: None,
    }
}

/// Start a full node with Substrate networking enabled.
///
/// Returns the client, task manager, and the node's local peer ID with listen address.
pub async fn start_seeder_node_with_networking(
    config: &mut Configuration,
    blocks_to_import: u32,
) -> Result<(Arc<FullClient>, TaskManager, MultiaddrWithPeerId), ServiceError> {
    let bitcoin_network = bitcoin::Network::Bitcoin;

    let NodeComponents {
        client,
        backend,
        mut task_manager,
        ..
    } = subcoin_service::new_node(SubcoinConfiguration::test_config(config))?;

    // Import blocks
    let mut bitcoin_block_import =
        BitcoinBlockImporter::<_, _, _, _, subcoin_service::TransactionAdapter>::new(
            client.clone(),
            client.clone(),
            ImportConfig {
                network: bitcoin_network,
                block_verification: BlockVerification::None,
                execute_block: true,
                script_engine: ScriptEngine::Core,
            },
            Arc::new(subcoin_service::CoinStorageKey),
            None,
        );

    let test_blocks = block_data();
    for block_number in 1..=blocks_to_import {
        let block = test_blocks[block_number as usize].clone();
        let import_status = bitcoin_block_import
            .import_block(block, BlockOrigin::Own)
            .await
            .map_err(|e| ServiceError::Other(format!("Block import failed: {e:?}")))?;
        assert!(matches!(import_status, ImportStatus::Imported { .. }));
    }

    // Start Substrate networking
    let (_system_rpc_tx, _sync_service) =
        subcoin_service::start_substrate_network::<sc_network::Litep2pNetworkBackend>(
            config,
            client.clone(),
            backend,
            &mut task_manager,
            bitcoin_network,
            None,
        )?;

    // Get the local peer ID and listen address
    let local_peer_id = config
        .network
        .node_key
        .clone()
        .into_keypair()
        .map_err(|e| ServiceError::Other(format!("Failed to get keypair: {e:?}")))?
        .public()
        .to_peer_id();

    let listen_addr = config
        .network
        .listen_addresses
        .first()
        .cloned()
        .ok_or_else(|| ServiceError::Other("No listen address".to_string()))?;

    let multiaddr_with_peer_id = MultiaddrWithPeerId {
        multiaddr: listen_addr,
        peer_id: local_peer_id,
    };

    Ok((client, task_manager, multiaddr_with_peer_id))
}

/// Result of starting a state syncer node.
pub struct StateSyncerNode {
    pub client: Arc<FullClient>,
    pub task_manager: TaskManager,
    pub multiaddr_with_peer_id: MultiaddrWithPeerId,
    /// Receiver for state sync completion notification.
    pub sync_complete_rx: tokio::sync::oneshot::Receiver<bool>,
}

/// Start a node configured for state sync.
///
/// This node will attempt to state sync from the specified target block
/// by connecting to the boot nodes.
pub async fn start_state_syncer_node(
    config: &mut Configuration,
    target_block_number: u32,
) -> Result<StateSyncerNode, ServiceError> {
    let bitcoin_network = bitcoin::Network::Bitcoin;

    // Set sync mode to LightState for state sync
    config.network.sync_mode = SyncMode::LightState {
        skip_proofs: true, // Skip proofs for faster testing
        storage_chain_mode: false,
    };

    // State sync requires pruned state, not archive
    config.state_pruning = Some(PruningMode::blocks_pruning(256));

    let NodeComponents {
        client,
        backend,
        mut task_manager,
        ..
    } = subcoin_service::new_node(SubcoinConfiguration::test_config(config))?;

    // Create channel for state sync completion notification
    let (sync_complete_tx, sync_complete_rx) = tokio::sync::oneshot::channel();

    // Start networking with state sync strategy
    start_state_sync_network::<sc_network::Litep2pNetworkBackend>(
        config,
        client.clone(),
        backend,
        &mut task_manager,
        bitcoin_network,
        target_block_number,
        sync_complete_tx,
    )?;

    // Get the local peer ID and listen address
    let local_peer_id = config
        .network
        .node_key
        .clone()
        .into_keypair()
        .map_err(|e| ServiceError::Other(format!("Failed to get keypair: {e:?}")))?
        .public()
        .to_peer_id();

    let listen_addr = config
        .network
        .listen_addresses
        .first()
        .cloned()
        .ok_or_else(|| ServiceError::Other("No listen address".to_string()))?;

    let multiaddr_with_peer_id = MultiaddrWithPeerId {
        multiaddr: listen_addr,
        peer_id: local_peer_id,
    };

    Ok(StateSyncerNode {
        client,
        task_manager,
        multiaddr_with_peer_id,
        sync_complete_rx,
    })
}

/// Start networking configured for state sync.
fn start_state_sync_network<N>(
    config: &mut Configuration,
    client: Arc<FullClient>,
    _backend: Arc<subcoin_service::FullBackend>,
    task_manager: &mut TaskManager,
    bitcoin_network: bitcoin::Network,
    target_block_number: u32,
    sync_complete_tx: tokio::sync::oneshot::Sender<bool>,
) -> Result<(), ServiceError>
where
    N: NetworkBackend<subcoin_runtime::interface::OpaqueBlock, sp_core::H256>,
{
    use sc_consensus::import_queue::BasicQueue;
    use sc_consensus_nakamoto::SubstrateImportQueueVerifier;
    use subcoin_runtime::interface::OpaqueBlock as Block;

    let mut net_config = sc_network::config::FullNetworkConfiguration::<
        Block,
        <Block as BlockT>::Hash,
        N,
    >::new(&config.network, config.prometheus_registry().cloned());

    let transaction_pool = Arc::from(subcoin_service::transaction_pool::new_dummy_pool());

    let import_queue = BasicQueue::new(
        SubstrateImportQueueVerifier::new(client.clone(), bitcoin_network),
        Box::new(client.clone()),
        None,
        &task_manager.spawn_essential_handle(),
        None,
    );

    let metrics = N::register_notification_metrics(config.prometheus_registry());

    let block_announce_validator =
        Box::new(sp_consensus::block_validation::DefaultBlockAnnounceValidator);

    let network_service_provider = NetworkServiceProvider::new();
    let protocol_id = config.protocol_id();
    let fork_id = config.chain_spec.fork_id();
    let metrics_registry = config.prometheus_config.as_ref().map(|c| &c.registry);

    let spawn_handle = task_manager.spawn_handle();

    // Add network request handler for subcoin protocol
    let network_request_protocol_config = {
        let (handler, protocol_config) =
            subcoin_service::network_request_handler::NetworkRequestHandler::new::<N>(
                &protocol_id,
                fork_id,
                client.clone(),
                100,
            );
        spawn_handle.spawn(
            "subcoin-network-request-handler",
            Some("networking"),
            handler.run(),
        );
        protocol_config
    };
    net_config.add_request_response_protocol(network_request_protocol_config);

    let block_downloader = sc_service::build_default_block_downloader(
        &protocol_id,
        fork_id,
        &mut net_config,
        network_service_provider.handle(),
        client.clone(),
        100,
        &spawn_handle,
    );

    // Build the standard Polkadot syncing strategy which supports state sync
    let syncing_strategy = sc_service::build_polkadot_syncing_strategy(
        protocol_id.clone(),
        fork_id,
        &mut net_config,
        None, // No warp sync provider
        block_downloader,
        client.clone(),
        &spawn_handle,
        config.prometheus_registry(),
    )?;

    let (syncing_engine, sync_service, block_announce_config) = SyncingEngine::new(
        Roles::from(&config.role),
        Arc::clone(&client),
        metrics_registry,
        metrics.clone(),
        &net_config,
        protocol_id.clone(),
        fork_id,
        block_announce_validator,
        syncing_strategy,
        network_service_provider.handle(),
        import_queue.service(),
        net_config.peer_store_handle(),
    )?;

    spawn_handle.spawn_blocking("syncing", None, syncing_engine.run());

    let (network, _system_rpc_tx, _tx_handler_controller, sync_service) =
        sc_service::build_network_advanced(sc_service::BuildNetworkAdvancedParams {
            role: config.role,
            protocol_id,
            fork_id,
            ipfs_server: false,
            announce_block: true,
            net_config,
            client: client.clone(),
            transaction_pool,
            spawn_handle: task_manager.spawn_handle(),
            import_queue,
            sync_service,
            block_announce_config,
            network_service_provider,
            metrics_registry: None,
            metrics,
        })?;

    // Spawn informant
    task_manager.spawn_handle().spawn(
        "substrate-informant",
        None,
        sc_informant::build(client.clone(), Arc::new(network), sync_service.clone()),
    );

    // Spawn a task to monitor sync progress and notify when complete
    let sync_client = client.clone();
    task_manager
        .spawn_handle()
        .spawn("sync-monitor", None, async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                let info = sync_client.info();
                tracing::debug!(
                    "Sync progress: best={}, target={}",
                    info.best_number,
                    target_block_number
                );
                if info.best_number >= target_block_number {
                    tracing::info!("State sync complete! Reached block {}", info.best_number);
                    let _ = sync_complete_tx.send(true);
                    break;
                }
            }
        });

    Ok(())
}
