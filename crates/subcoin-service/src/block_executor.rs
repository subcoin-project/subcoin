use crate::{
    initialize_genesis_block_hash_mapping, CoinStorageKey, FullBackend, FullClient,
    GenesisBlockBuilder, InMemoryBackend, InMemoryClient, SubcoinExecutorDispatch,
    TransactionAdapter,
};
use sc_client_api::{Backend, HeaderBackend, StateBackend, StorageProvider};
use sc_consensus_nakamoto::{
    BlockExecutionStrategy, BlockExecutor, ClientContext, ExecutionBackend,
};
use sc_executor::NativeElseWasmExecutor;
use sc_service::{Configuration, Error as ServiceError, SpawnTaskHandle};
use sp_runtime::traits::{Block as BlockT, HashingFor, Header as HeaderT};
use sp_runtime::StateVersion;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use subcoin_runtime::interface::OpaqueBlock as Block;

fn runtime_hash_and_code(
    backend: &Arc<FullBackend>,
    block_hash: <Block as BlockT>::Hash,
) -> (<Block as BlockT>::Hash, Vec<u8>) {
    let state = backend.state_at(block_hash).expect("State not found");

    let hash = state
        .storage_hash(sp_core::storage::well_known_keys::CODE)
        .ok()
        .flatten()
        .expect("`:code` hash not found");

    let code = state
        .storage(sp_core::storage::well_known_keys::CODE)
        .ok()
        .flatten()
        .expect("code not found");

    (hash, code)
}

fn new_in_memory_backend(
    client: &Arc<FullClient>,
    backend: &Arc<FullBackend>,
) -> Result<(InMemoryBackend, bool), ServiceError> {
    let info = client.info();

    let best_number = info.best_number;
    let best_hash = info.best_hash;

    // Read the runtime code from the disk backend and use them to initialize the fast-sync
    // backend.
    let (runtime_hash, runtime_code) = runtime_hash_and_code(backend, best_hash);

    let mut in_memory_backend = sc_fast_sync_backend::Backend::new(runtime_hash, runtime_code);

    let mut is_refresh = true;

    if best_number > 0u32 {
        tracing::info!(
            "Initializing in-memory backend from state at #{best_number},{best_hash}.{}",
            if best_number > 150_000 {
                " Please wait. This might take some time depending on the state size."
            } else {
                ""
            }
        );

        let now = std::time::Instant::now();

        // This might take long when the chain state is huge.
        let top = client
            .storage_pairs(info.best_hash, None, None)?
            .map(|(key, data)| (key.0, data.0))
            .collect::<BTreeMap<_, _>>();

        let storage = sp_storage::Storage {
            top,
            ..Default::default()
        };

        let chain_state: sp_state_machine::InMemoryBackend<HashingFor<Block>> =
            (storage, StateVersion::V0).into();

        let best_header = client.header(best_hash)?.expect("Best header must exist");

        let state_root = *best_header.state_root();

        in_memory_backend.initialize(best_header, info.genesis_hash, chain_state);

        assert_eq!(
            state_root,
            in_memory_backend.storage_root(),
            "Storage root of initialized in-memory backend must match the state root"
        );

        is_refresh = false;

        tracing::info!(
            "In memory backend initialized successfully in {}ms",
            now.elapsed().as_millis()
        );
    }

    Ok((in_memory_backend, is_refresh))
}

pub(super) fn new_in_memory_client(
    client: Arc<FullClient>,
    backend: Arc<FullBackend>,
    executor: NativeElseWasmExecutor<SubcoinExecutorDispatch>,
    bitcoin_network: bitcoin::Network,
    spawn_handle: SpawnTaskHandle,
    config: &Configuration,
) -> Result<Arc<InMemoryClient>, ServiceError> {
    let (in_memory_backend, is_refresh) = new_in_memory_backend(&client, &backend)?;
    let in_memory_backend = Arc::new(in_memory_backend);

    let no_genesis = !is_refresh;

    let genesis_block_builder = GenesisBlockBuilder::<_, _, _, TransactionAdapter>::new(
        bitcoin_network,
        config.chain_spec.as_storage_builder(),
        !no_genesis,
        in_memory_backend.clone(),
        executor.clone(),
    )?;

    let client_config = sc_service::ClientConfig {
        offchain_worker_enabled: config.offchain_worker.enabled,
        offchain_indexing_api: config.offchain_worker.indexing_enabled,
        wasm_runtime_overrides: config.wasm_runtime_overrides.clone(),
        no_genesis,
        wasm_runtime_substitutes: HashMap::new(),
        enable_import_proof_recording: false,
    };

    let in_memory_client = sc_service::client::new_with_backend(
        in_memory_backend,
        executor,
        genesis_block_builder,
        Box::new(spawn_handle),
        None,
        None,
        client_config,
    )?;

    initialize_genesis_block_hash_mapping(&in_memory_client, bitcoin_network);

    Ok(Arc::new(in_memory_client))
}

fn new_box<T: BlockExecutor<Block> + 'static>(processor: T) -> Box<dyn BlockExecutor<Block>> {
    Box::new(processor) as Box<dyn BlockExecutor<Block>>
}

pub(super) fn new_block_executor(
    client: Arc<FullClient>,
    block_execution_strategy: BlockExecutionStrategy,
    in_memory_client: Option<Arc<InMemoryClient>>,
) -> Box<dyn BlockExecutor<Block>> {
    use sc_consensus_nakamoto::{
        BenchmarkAllExecutor, BenchmarkRuntimeBlockExecutor, OffRuntimeBlockExecutor,
        RuntimeBlockExecutor,
    };

    match block_execution_strategy {
        BlockExecutionStrategy::RuntimeExecution(exec_backend) => match exec_backend {
            ExecutionBackend::Disk => new_box(RuntimeBlockExecutor::new(
                client.clone(),
                ClientContext::<FullClient>::Disk,
            )),
            ExecutionBackend::InMemory => {
                let in_memory_client = in_memory_client.expect("In memory create must be created");
                new_box(RuntimeBlockExecutor::new(
                    in_memory_client.clone(),
                    ClientContext::InMemory(in_memory_client),
                ))
            }
        },
        BlockExecutionStrategy::OffRuntimeExecution(exec_backend) => match exec_backend {
            ExecutionBackend::Disk => {
                new_box(
                    OffRuntimeBlockExecutor::<_, _, _, TransactionAdapter, _>::new(
                        client.clone(),
                        ClientContext::<FullClient>::Disk,
                        Arc::new(CoinStorageKey),
                    ),
                )
            }
            ExecutionBackend::InMemory => {
                let in_memory_client = in_memory_client.expect("In memory create must be created");
                new_box(
                    OffRuntimeBlockExecutor::<_, _, _, TransactionAdapter, _>::new(
                        in_memory_client.clone(),
                        ClientContext::InMemory(in_memory_client),
                        Arc::new(CoinStorageKey),
                    ),
                )
            }
        },
        BlockExecutionStrategy::BenchmarkRuntimeExecution => {
            let disk_runtime_block_executor = new_box(RuntimeBlockExecutor::new(
                client.clone(),
                ClientContext::<FullClient>::Disk,
            ));
            let in_memory_client = in_memory_client.expect("In memory create must be created");
            let in_memory_runtime_block_executor = new_box(RuntimeBlockExecutor::new(
                in_memory_client.clone(),
                ClientContext::InMemory(in_memory_client),
            ));
            new_box(BenchmarkRuntimeBlockExecutor::new(
                disk_runtime_block_executor,
                in_memory_runtime_block_executor,
            ))
        }
        BlockExecutionStrategy::BenchmarkAll => {
            let disk_runtime_block_executor =
                RuntimeBlockExecutor::new(client.clone(), ClientContext::<FullClient>::Disk);
            let in_memory_client = in_memory_client.expect("In memory create must be created");
            let in_memory_runtime_block_executor = RuntimeBlockExecutor::new(
                in_memory_client.clone(),
                ClientContext::InMemory(in_memory_client.clone()),
            );
            let disk_off_runtime_block_executor =
                OffRuntimeBlockExecutor::<_, _, _, TransactionAdapter, _>::new(
                    client.clone(),
                    ClientContext::<FullClient>::Disk,
                    Arc::new(CoinStorageKey),
                );
            let in_memory_off_runtime_block_executor =
                OffRuntimeBlockExecutor::<_, _, _, TransactionAdapter, _>::new(
                    in_memory_client.clone(),
                    ClientContext::InMemory(in_memory_client),
                    Arc::new(CoinStorageKey),
                );
            new_box(BenchmarkAllExecutor::new(
                disk_runtime_block_executor,
                in_memory_runtime_block_executor,
                disk_off_runtime_block_executor,
                in_memory_off_runtime_block_executor,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{new_node, NodeComponents, SubcoinConfiguration};
    use bitcoin::consensus::Encodable;
    use sc_consensus_nakamoto::{
        BitcoinBlockImport, BitcoinBlockImporter, BlockVerification, ImportConfig, ImportStatus,
    };
    use sc_service::config::DatabaseSource;
    use sc_service::BasePath;
    use sp_core::Encode;
    use subcoin_test_service::{block_data, new_test_node_and_produce_blocks};
    use tokio::runtime::Handle;

    #[tokio::test]
    #[ignore]
    async fn inspect_substrate_header_size() {
        let runtime_handle = Handle::current();
        let config = subcoin_test_service::test_configuration(runtime_handle);
        let NodeComponents { client, .. } =
            new_node(SubcoinConfiguration::test_config(&config)).expect("Failed to create node");

        let substrate_genesis_header = client.header(client.info().genesis_hash).unwrap().unwrap();
        let bitcoin_genesis_header =
            bitcoin::constants::genesis_block(bitcoin::Network::Bitcoin).header;

        let encoded_substrate_header = substrate_genesis_header.encode();
        let mut encoded_bitcoin_header = Vec::new();
        bitcoin_genesis_header
            .consensus_encode(&mut encoded_bitcoin_header)
            .unwrap();

        println!(
            "encoded_substrate_header: {} bytes",
            encoded_substrate_header.len()
        );
        println!(
            "parent_hash: {}, number: {}, state_root: {}, extrinsics_root: {}, digest: {}",
            substrate_genesis_header.parent_hash().encode().len(),
            substrate_genesis_header.number().encode().len(),
            substrate_genesis_header.state_root().encode().len(),
            substrate_genesis_header.extrinsics_root().encode().len(),
            substrate_genesis_header.digest().encode().len(),
        );

        for d in substrate_genesis_header.digest().logs.iter() {
            println!("digest item: {}", d.encode().len());
        }

        println!(
            "encoded_bitcoin_header: {} bytes",
            encoded_bitcoin_header.len()
        );
    }

    #[tokio::test]
    async fn off_runtime_in_memory_executor_should_produce_same_result_as_runtime_disk_executor() {
        sp_tracing::try_init_simple();

        let runtime_handle = Handle::current();

        let network = bitcoin::Network::Bitcoin;

        let mut config = subcoin_test_service::test_configuration(runtime_handle);

        let client = new_test_node_and_produce_blocks(&config, 3).await;
        let expected_header3 = client.header(client.info().best_hash).unwrap().unwrap();

        // Use a different data path as the client above may still hold the database file.
        let tmp = tempfile::tempdir().unwrap();
        let base_path = BasePath::new(tmp.path());
        let root = base_path.path().to_path_buf();
        config.database = DatabaseSource::ParityDb {
            path: root.join("db"),
        };
        config.data_path = base_path.path().into();
        config.base_path = base_path;

        let NodeComponents {
            block_executor,
            client,
            backend,
            task_manager,
            executor,
            ..
        } = new_node(SubcoinConfiguration::test_config(&config)).expect("Failed to create node");

        let mut bitcoin_block_import = BitcoinBlockImporter::<_, _, _, _, TransactionAdapter>::new(
            client.clone(),
            client.clone(),
            ImportConfig {
                network,
                block_verification: BlockVerification::None,
                execute_block: true,
                verify_script: true,
            },
            Arc::new(CoinStorageKey),
            block_executor,
            None,
        );

        let test_blocks = block_data();

        bitcoin_block_import
            .import_block(test_blocks[1].clone())
            .await
            .unwrap();

        let in_mem_client = new_in_memory_client(
            client.clone(),
            backend.clone(),
            executor.clone(),
            network,
            task_manager.spawn_handle(),
            &config,
        )
        .unwrap();

        let block_executor = new_block_executor(
            client.clone(),
            BlockExecutionStrategy::off_runtime_in_memory(),
            Some(in_mem_client),
        );

        // Set the block executor using in memory backend initialized from the latest state in the
        // disk backend.
        bitcoin_block_import.set_block_executor(block_executor);

        let import_status = bitcoin_block_import
            .import_block(test_blocks[2].clone())
            .await
            .unwrap();
        assert!(matches!(import_status, ImportStatus::Imported { .. }));

        let import_status = bitcoin_block_import
            .import_block(test_blocks[3].clone())
            .await
            .unwrap();
        assert!(matches!(import_status, ImportStatus::Imported { .. }));

        let best_header = client.header(client.info().best_hash).unwrap().unwrap();

        assert_eq!(best_header, expected_header3);

        let _ = tmp.close();
    }
}
