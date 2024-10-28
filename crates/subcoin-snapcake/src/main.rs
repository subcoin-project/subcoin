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
mod network;
mod params;

use self::cli::{App, Command};
use clap::Parser;
use sc_cli::SubstrateCli;
use sc_consensus::import_queue::BasicQueue;
use sc_consensus_nakamoto::SubstrateImportQueueVerifier;
use sc_executor::WasmExecutor;
use sc_network::config::NetworkBackendType;
use sc_service::{Configuration, TaskManager};
use sp_runtime::traits::Block as BlockT;
use std::path::PathBuf;
use std::sync::Arc;
use subcoin_runtime::interface::OpaqueBlock as Block;
use subcoin_runtime::RuntimeApi;
use subcoin_service::{GenesisBlockBuilder, TransactionAdapter};

type FullClient = sc_service::TFullClient<Block, RuntimeApi, WasmExecutor>;

fn main() -> sc_cli::Result<()> {
    let app = App::parse();

    let bitcoin_network = app.chain.bitcoin_network();
    let skip_proof = app.skip_proof;

    let command = Command::new(app);

    cli::SubstrateCli
        .create_runner(&command)?
        .run_node_until_exit(|config| async move {
            start_snapcake_node(bitcoin_network, config, skip_proof, command.snapshot_dir())
        })
        .map_err(Into::into)
}

fn start_snapcake_node(
    bitcoin_network: bitcoin::Network,
    mut config: Configuration,
    skip_proof: bool,
    snapshot_dir: PathBuf,
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

    let syncing_strategy = crate::network::build_snapcake_syncing_strategy(
        config.protocol_id(),
        config.chain_spec.fork_id(),
        &mut net_config,
        client.clone(),
        &task_manager.spawn_handle(),
        skip_proof,
        snapshot_dir,
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
