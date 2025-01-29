use bitcoin::consensus::Decodable;
use bitcoin::hex::FromHex;
use bitcoin::Block;
use sc_consensus_nakamoto::{
    BitcoinBlockImport, BitcoinBlockImporter, BlockVerification, ImportConfig, ImportStatus,
    ScriptEngine,
};
use sc_service::config::{
    BlocksPruning, DatabaseSource, ExecutorConfiguration, KeystoreConfig, NetworkConfiguration,
    OffchainWorkerConfig, PruningMode, RpcBatchRequestConfig, RpcConfiguration,
    WasmExecutionMethod, WasmtimeInstantiationStrategy,
};
use sc_service::error::Error as ServiceError;
use sc_service::{BasePath, ChainSpec, Configuration, Role};
use sp_consensus::BlockOrigin;
use sp_keyring::sr25519::Keyring as Sr25519Keyring;
use std::sync::Arc;
use subcoin_service::{FullClient, NodeComponents, SubcoinConfiguration};

fn decode_raw_block(hex_str: &str) -> Block {
    let data = Vec::<u8>::from_hex(hex_str).expect("Failed to convert hex str");
    Block::consensus_decode(&mut data.as_slice()).expect("Failed to convert hex data to Block")
}

pub fn block_data() -> Vec<Block> {
    // genesis block
    let block0 = decode_raw_block("0100000000000000000000000000000000000000000000000000000000000000000000003ba3edfd7a7b12b27ac72c3e67768f617fc81bc3888a51323a9fb8aa4b1e5e4a29ab5f49ffff001d1dac2b7c0101000000010000000000000000000000000000000000000000000000000000000000000000ffffffff4d04ffff001d0104455468652054696d65732030332f4a616e2f32303039204368616e63656c6c6f72206f6e206272696e6b206f66207365636f6e64206261696c6f757420666f722062616e6b73ffffffff0100f2052a01000000434104678afdb0fe5548271967f1a67130b7105cd6a828e03909a67962e0ea1f61deb649f6bc3f4cef38c4f35504e51ec112de5c384df7ba0b8d578a4c702b6bf11d5fac00000000");
    // https://webbtc.com/block/00000000839a8e6886ab5951d76f411475428afc90947ee320161bbf18eb6048
    // height 1
    let block1 = decode_raw_block("010000006fe28c0ab6f1b372c1a6a246ae63f74f931e8365e15a089c68d6190000000000982051fd1e4ba744bbbe680e1fee14677ba1a3c3540bf7b1cdb606e857233e0e61bc6649ffff001d01e362990101000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0704ffff001d0104ffffffff0100f2052a0100000043410496b538e853519c726a2c91e61ec11600ae1390813a627c66fb8be7947be63c52da7589379515d4e0a604f8141781e62294721166bf621e73a82cbf2342c858eeac00000000");
    // https://webbtc.com/block/000000006a625f06636b8bb6ac7b960a8d03705d1ace08b1a19da3fdcc99ddbd.hex
    // height 2
    let block2  = decode_raw_block("010000004860eb18bf1b1620e37e9490fc8a427514416fd75159ab86688e9a8300000000d5fdcc541e25de1c7a5addedf24858b8bb665c9f36ef744ee42c316022c90f9bb0bc6649ffff001d08d2bd610101000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0704ffff001d010bffffffff0100f2052a010000004341047211a824f55b505228e4c3d5194c1fcfaa15a456abdf37f9b9d97a4040afc073dee6c89064984f03385237d92167c13e236446b417ab79a0fcae412ae3316b77ac00000000");
    // https://webbtc.com/block/0000000082b5015589a3fdf2d4baff403e6f0be035a5d9742c1cae6295464449.hex
    // height 3
    let block3 = decode_raw_block("01000000bddd99ccfda39da1b108ce1a5d70038d0a967bacb68b6b63065f626a0000000044f672226090d85db9a9f2fbfe5f0f9609b387af7be5b7fbb7a1767c831c9e995dbe6649ffff001d05e0ed6d0101000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0704ffff001d010effffffff0100f2052a0100000043410494b9d3e76c5b1629ecf97fff95d7a4bbdac87cc26099ada28066c6ff1eb9191223cd897194a08d0c2726c5747f1db49e8cf90e75dc3e3550ae9b30086f3cd5aaac00000000");
    vec![block0, block1, block2, block3]
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
            "Can not produce too many blocks (maximum: {}) in test env",
            test_blocks.len()
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
