//! # subcoin-snapcake
//!
//! `snapcake` is a Decentralized Bitcoin UTXO Set Snapshot Provider. It functions as a lightweight
//! Substrate-based node specifically designed for downloading the UTXO set directly from the Subcoin
//! P2P network without requiring the full functionality of a regular Substrate node. The other
//! node components are only included to fulfill the structural requirements of building an instance
//! of [`sc_service::Client`] but do not perform any significant tasks.
//!
//! The downloaded UTXO set snapshot is fully compatible with Bitcoin Core, enabling Bitcoin Core's
//! Fast Sync feature without the need for a trusted snapshot provider. This enhances decentralization
//! by allowing users to quickly synchronize their Bitcoin Core node without relying on external,
//! centralized sources for the snapshot.
//!
//! ## Features
//!
//! - **Decentralized UTXO Set Download:** Fetch the UTXO set from the decentralized Subcoin P2P network,
//!   removing the need for trusted third-party snapshots.
//! - **Bitcoin Core Compatibility:** Generate UTXO set snapshots compatible with Bitcoin Core’s `txoutset`,
//!   allowing for seamless integration with Bitcoin’s Fast Sync functionality.
//! - **Lightweight Node:** Only includes essential network components for state syncing, reducing
//!   resource consumption compared to a full regular Substrate node.
//!
//! ## Custom Syncing Strategy
//!
//! The `SnapcakeSyncingStrategy` is a customized version of the `PolkadotSyncingStrategy`. It queries the
//! best header from connected peers and begins the state sync process targeting the best block header.
//! During state sync, responses are intercepted to extract UTXO data, which is then saved locally. Once the
//! state sync completes, a UTXO set snapshot compatible with Bitcoin Core's `txoutset` is generated.
//!
//! ## Use Case
//!
//! `snapcake` can be used to speed up the synchronization of Bitcoin Core nodes by providing decentralized
//! UTXO snapshots, thereby reducing the reliance on centralized snapshot providers and enhancing the
//! decentralization of Bitcoin’s fast sync process.

mod cli;
mod params;
mod snapshot_manager;
mod state_sync_wrapper;
mod syncing_strategy;

use self::cli::{App, Command};
use clap::Parser;
use sc_cli::SubstrateCli;
use sc_consensus::import_queue::BasicQueue;
use sc_consensus_nakamoto::SubstrateImportQueueVerifier;
use sc_executor::WasmExecutor;
use sc_network::config::NetworkBackendType;
use sc_network::Roles;
use sc_network_sync::engine::SyncingEngine;
use sc_network_sync::service::network::NetworkServiceProvider;
use sc_service::{Configuration, ImportQueue, TaskManager};
use sp_runtime::traits::Block as BlockT;
use std::path::PathBuf;
use std::sync::Arc;
use subcoin_runtime::interface::OpaqueBlock as Block;
use subcoin_runtime::RuntimeApi;
use subcoin_service::{GenesisBlockBuilder, TransactionAdapter};
use syncing_strategy::TargetBlock;

type FullClient = sc_service::TFullClient<Block, RuntimeApi, WasmExecutor>;

fn main() -> sc_cli::Result<()> {
    let app = App::parse();

    let bitcoin_network = app.chain.bitcoin_network();
    let skip_proof = app.skip_proof;
    let sync_target = if let Some(number) = app.network_params.block_number {
        TargetBlock::Number(number)
    } else if let Some(hash) = app.network_params.block_hash {
        TargetBlock::Hash(hash)
    } else {
        TargetBlock::LastFinalized
    };

    let command = Command::new(app);

    cli::SubstrateCli
        .create_runner(&command)?
        .run_node_until_exit(|config| async move {
            start_snapcake_node(
                bitcoin_network,
                config,
                skip_proof,
                command.snapshot_dir(),
                sync_target,
            )
        })
        .map_err(Into::into)
}

fn start_snapcake_node(
    bitcoin_network: bitcoin::Network,
    mut config: Configuration,
    skip_proof: bool,
    snapshot_dir: PathBuf,
    sync_target: TargetBlock<Block>,
) -> Result<TaskManager, sc_service::error::Error> {
    let executor = sc_service::new_wasm_executor(&config.executor);
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

    match config.network.network_backend {
        NetworkBackendType::Libp2p => {
            start_substrate_network::<sc_network::NetworkWorker<Block, <Block as BlockT>::Hash>>(
                &mut config,
                client,
                &mut task_manager,
                bitcoin_network,
                skip_proof,
                snapshot_dir,
                sync_target,
            )?
        }
        NetworkBackendType::Litep2p => {
            start_substrate_network::<sc_network::Litep2pNetworkBackend>(
                &mut config,
                client,
                &mut task_manager,
                bitcoin_network,
                skip_proof,
                snapshot_dir,
                sync_target,
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
    skip_proof: bool,
    snapshot_dir: PathBuf,
    sync_target: TargetBlock<Block>,
) -> Result<(), sc_service::error::Error>
where
    N: sc_network::NetworkBackend<Block, <Block as BlockT>::Hash>,
{
    let mut net_config = sc_network::config::FullNetworkConfiguration::<
        Block,
        <Block as BlockT>::Hash,
        N,
    >::new(&config.network, None);

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
    let metrics_registry = config
        .prometheus_config
        .as_ref()
        .map(|config| &config.registry);

    let spawn_handle = task_manager.spawn_handle();

    let block_downloader = sc_service::build_default_block_downloader(
        &protocol_id,
        fork_id,
        &mut net_config,
        network_service_provider.handle(),
        client.clone(),
        100,
        &spawn_handle,
    );

    let syncing_strategy = crate::syncing_strategy::build_snapcake_syncing_strategy(
        protocol_id.clone(),
        fork_id,
        &mut net_config,
        client.clone(),
        &spawn_handle,
        block_downloader,
        skip_proof,
        snapshot_dir,
        sync_target,
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
            announce_block: false,
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

    spawn_handle.spawn(
        "substrate-informant",
        None,
        sc_informant::build(client.clone(), Arc::new(network), sync_service.clone()),
    );

    Ok(())
}

