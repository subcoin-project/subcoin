//! # substrate-state-sync-node
//!
//! Substrate State Sync Node is essentially a stripped-down Substrate node that only includes the
//! network component for downloading the state directly from the P2P network. Other components are
//! constructed solely to satisfy the requirements for building an instance of [`sc_service::Client`]
//! and don't do any substantial work.
//!
//! ## Custom Syncing Strategy
//!
//! The `SubcoinSyncingStrategy` is adapted from the `PolkadotSyncingStrategy`. It requests the best
//! header from peers and initiates a state sync targeting the best header. Once the state sync is
//! complete, a UTXO set snapshot compatiable with Bitcoin Core txoutset will be produced.

mod cli;
mod network;
mod params;

use self::cli::{App, Command};
use clap::Parser;
use sc_cli::SubstrateCli;
use sc_consensus::import_queue::BasicQueue;
use sc_consensus_nakamoto::SubstrateImportQueueVerifier;
use sc_network::config::NetworkBackendType;
use sc_service::{Configuration, TaskManager};
use sp_runtime::traits::Block as BlockT;
use std::sync::Arc;
use subcoin_runtime::interface::OpaqueBlock as Block;
use subcoin_runtime::RuntimeApi;
use subcoin_service::{FullClient, GenesisBlockBuilder, TransactionAdapter};

fn main() -> sc_cli::Result<()> {
    let app = App::parse();

    let bitcoin_network = app.chain.bitcoin_network();
    let skip_proof = app.skip_proof;

    let command = Command::new(app);

    cli::SubstrateCli
        .create_runner(&command)?
        .run_node_until_exit(
            |config| async move { start_node(bitcoin_network, config, skip_proof) },
        )
        .map_err(Into::into)
}

fn start_node(
    bitcoin_network: bitcoin::Network,
    mut config: Configuration,
    skip_proof: bool,
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
        NetworkBackendType::Libp2p => {
            start_substrate_network::<sc_network::NetworkWorker<Block, <Block as BlockT>::Hash>>(
                &mut config,
                client,
                &mut task_manager,
                bitcoin_network,
                skip_proof,
            )?
        }
        NetworkBackendType::Litep2p => {
            start_substrate_network::<sc_network::Litep2pNetworkBackend>(
                &mut config,
                client,
                &mut task_manager,
                bitcoin_network,
                skip_proof,
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

    let syncing_strategy = crate::network::build_subcoin_syncing_strategy(
        config.protocol_id(),
        config.chain_spec.fork_id(),
        &mut net_config,
        client.clone(),
        &task_manager.spawn_handle(),
        skip_proof,
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
