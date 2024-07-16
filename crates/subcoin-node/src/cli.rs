pub mod params;

use crate::commands::blockchain::{Blockchain, BlockchainCmd};
use crate::commands::import_blocks::{ImportBlocks, ImportBlocksCmd};
use crate::commands::run::{Run, RunCmd};
use crate::commands::tools::Tools;
use crate::substrate_cli::SubstrateCli;
use clap::Parser;
use frame_benchmarking_cli::{BenchmarkCmd, SUBSTRATE_REFERENCE_HARDWARE};
use sc_cli::SubstrateCli as SubstrateCliT;
use sc_client_api::UsageProvider;
use sc_consensus_nakamoto::{BitcoinBlockImporter, ImportConfig};
use sc_service::PartialComponents;
use std::sync::Arc;

// 6 blocks is the standard confirmation period in the Bitcoin community.
const CONFIRMATION_DEPTH: u32 = 6u32;

#[derive(Debug, clap::Subcommand)]
pub enum Command {
    /// Run subcoin node.
    Run(Run),

    /// Import blocks.
    ImportBlocks(ImportBlocks),

    /// Utility tools.
    #[command(subcommand)]
    Tools(Tools),

    /// Blockchain.
    #[command(subcommand)]
    Blockchain(Blockchain),

    /// Validate blocks.
    CheckBlock(Box<sc_cli::CheckBlockCmd>),

    /// Export blocks.
    ExportBlocks(sc_cli::ExportBlocksCmd),

    /// Export the state of a given block into a chain spec.
    ExportState(sc_cli::ExportStateCmd),

    /// Remove the whole chain.
    PurgeChain(sc_cli::PurgeChainCmd),

    /// Revert the chain to a previous state.
    Revert(sc_cli::RevertCmd),

    /// Sub-commands concerned with benchmarking.
    #[command(subcommand)]
    Benchmark(Box<frame_benchmarking_cli::BenchmarkCmd>),

    /// Db meta columns information.
    ChainInfo(sc_cli::ChainInfoCmd),
}

#[derive(Debug, Parser)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Disable automatic hardware benchmarks.
    ///
    /// By default these benchmarks are automatically ran at startup and measure
    /// the CPU speed, the memory bandwidth and the disk speed.
    ///
    /// The results are then printed out in the logs, and also sent as part of
    /// telemetry, if telemetry is enabled.
    #[arg(long)]
    pub no_hardware_benchmarks: bool,

    #[allow(missing_docs)]
    #[clap(flatten)]
    pub storage_monitor: sc_storage_monitor::StorageMonitorParams,
}

/// Parse and run command line arguments
pub fn run() -> sc_cli::Result<()> {
    let Cli {
        command,
        no_hardware_benchmarks,
        storage_monitor,
    } = Cli::parse();

    match command {
        Command::Run(run) => {
            let block_execution_strategy = run.common_params.block_execution_strategy();
            let no_finalizer = run.no_finalizer;
            let major_sync_confirmation_depth = run.major_sync_confirmation_depth;
            let run_cmd = RunCmd::new(&run);
            let runner = SubstrateCli.create_runner(&run_cmd)?;
            runner.run_node_until_exit(|config| async move {
                let network = run.common_params.bitcoin_network();

                let subcoin_service::NodeComponents {
                    client,
                    mut task_manager,
                    block_executor,
                    system_rpc_tx,
                    ..
                } = subcoin_service::new_node(subcoin_service::SubcoinConfiguration {
                    network,
                    config: &config,
                    block_execution_strategy,
                    no_hardware_benchmarks,
                    storage_monitor,
                })?;

                let chain_info = client.usage_info().chain;

                tracing::info!("📦 Highest known block at #{}", chain_info.best_number);

                let spawn_handle = task_manager.spawn_handle();

                let bitcoin_block_import =
                    BitcoinBlockImporter::<_, _, _, _, subcoin_service::TransactionAdapter>::new(
                        client.clone(),
                        client.clone(),
                        ImportConfig {
                            network,
                            block_verification: run.block_verification,
                            execute_block: true,
                        },
                        Arc::new(subcoin_service::CoinStorageKey),
                        block_executor,
                    );

                let import_queue = sc_consensus_nakamoto::bitcoin_import_queue(
                    &task_manager.spawn_essential_handle(),
                    bitcoin_block_import,
                );

                let (bitcoin_network, network_handle) = subcoin_network::Network::new(
                    client.clone(),
                    run.subcoin_network_params(network),
                    import_queue,
                    spawn_handle.clone(),
                );

                // TODO: handle Substrate networking and Bitcoin networking properly.
                task_manager.spawn_essential_handle().spawn_blocking(
                    "subcoin-networking",
                    None,
                    async move {
                        if let Err(err) = bitcoin_network.run().await {
                            tracing::error!(?err, "Subcoin network worker exited");
                        }
                    },
                );

                // TODO: Bitcoin-compatible RPC
                // Start JSON-RPC server.
                let gen_rpc_module = |deny_unsafe: sc_rpc::DenyUnsafe| {
                    let system_info = sc_rpc::system::SystemInfo {
                        chain_name: config.chain_spec.name().into(),
                        impl_name: config.impl_name.clone(),
                        impl_version: config.impl_version.clone(),
                        properties: config.chain_spec.properties(),
                        chain_type: config.chain_spec.chain_type(),
                    };
                    let system_rpc_tx = system_rpc_tx.clone();

                    crate::rpc::gen_rpc_module(
                        system_info,
                        client.clone(),
                        task_manager.spawn_handle(),
                        system_rpc_tx,
                        deny_unsafe,
                        network_handle.clone(),
                    )
                };

                let rpc = sc_service::start_rpc_servers(&config, gen_rpc_module, None)?;
                task_manager.keep_alive((config.base_path, rpc));

                if !no_finalizer {
                    spawn_handle.spawn("finalizer", None, {
                        subcoin_service::finalize_confirmed_blocks(
                            client.clone(),
                            spawn_handle.clone(),
                            CONFIRMATION_DEPTH,
                            major_sync_confirmation_depth,
                            network_handle.is_major_syncing(),
                        )
                    });
                }

                // Spawn subcoin informant task.
                spawn_handle.spawn(
                    "subcoin-informant",
                    None,
                    subcoin_informant::build(client.clone(), network_handle, Default::default()),
                );

                Ok(task_manager)
            })
        }
        Command::ImportBlocks(cmd) => {
            let block_execution_strategy = cmd.common_params.block_execution_strategy();
            let import_blocks_cmd = ImportBlocksCmd::new(&cmd);
            let runner = SubstrateCli.create_runner(&import_blocks_cmd)?;
            let data_dir = cmd.data_dir;
            runner.async_run(|config| {
                let subcoin_service::NodeComponents {
                    client,
                    task_manager,
                    block_executor,
                    ..
                } = subcoin_service::new_node(subcoin_service::SubcoinConfiguration {
                    network: bitcoin::Network::Bitcoin,
                    config: &config,
                    block_execution_strategy,
                    no_hardware_benchmarks,
                    storage_monitor,
                })?;
                task_manager.spawn_handle().spawn("finalizer", None, {
                    let client = client.clone();
                    let spawn_handle = task_manager.spawn_handle();
                    // Assume the chain is major syncing.
                    let is_major_syncing = Arc::new(true.into());
                    subcoin_service::finalize_confirmed_blocks(
                        client,
                        spawn_handle,
                        CONFIRMATION_DEPTH,
                        100,
                        is_major_syncing,
                    )
                });
                Ok((
                    import_blocks_cmd.run(client, block_executor, data_dir),
                    task_manager,
                ))
            })
        }
        Command::Tools(tools) => tools.run(),
        Command::Blockchain(blockchain) => {
            let block_execution_strategy = blockchain.block_execution_strategy();
            let cmd = BlockchainCmd::new(blockchain);
            let runner = SubstrateCli.create_runner(&cmd)?;
            runner.async_run(|config| {
                let subcoin_service::NodeComponents {
                    client,
                    task_manager,
                    ..
                } = subcoin_service::new_node(subcoin_service::SubcoinConfiguration {
                    network: bitcoin::Network::Bitcoin,
                    config: &config,
                    block_execution_strategy,
                    no_hardware_benchmarks: true,
                    storage_monitor,
                })?;
                Ok((cmd.run(client), task_manager))
            })
        }
        Command::CheckBlock(cmd) => {
            let runner = SubstrateCli.create_runner(&*cmd)?;
            runner.async_run(|config| {
                let PartialComponents {
                    client,
                    task_manager,
                    import_queue,
                    ..
                } = subcoin_service::new_partial(&config)?;
                Ok((cmd.run(client, import_queue), task_manager))
            })
        }
        Command::ExportBlocks(cmd) => {
            let runner = SubstrateCli.create_runner(&cmd)?;
            runner.async_run(|config| {
                let PartialComponents {
                    client,
                    task_manager,
                    ..
                } = subcoin_service::new_partial(&config)?;
                Ok((cmd.run(client, config.database), task_manager))
            })
        }
        Command::ExportState(cmd) => {
            let runner = SubstrateCli.create_runner(&cmd)?;
            runner.async_run(|config| {
                let PartialComponents {
                    client,
                    task_manager,
                    ..
                } = subcoin_service::new_partial(&config)?;

                let run_cmd = async move {
                    tracing::info!("Exporting raw state...");
                    // let block_id = cmd.input.as_ref().map(|b| b.parse()).transpose()?;
                    // let hash = match block_id {
                    // Some(id) => client.expect_block_hash_from_id(&id)?,
                    // None => client.usage_info().chain.best_hash,
                    let hash = client.usage_info().chain.best_hash;
                    // };
                    let _raw_state = sc_service::chain_ops::export_raw_state(client, hash).unwrap();

                    tracing::info!("Raw state exported successfully");

                    Ok(())
                };

                Ok((run_cmd, task_manager))
            })
        }
        Command::PurgeChain(cmd) => {
            let runner = SubstrateCli.create_runner(&cmd)?;
            runner.sync_run(|config| cmd.run(config.database))
        }
        Command::Revert(cmd) => {
            let runner = SubstrateCli.create_runner(&cmd)?;
            runner.async_run(|config| {
                let PartialComponents {
                    client,
                    task_manager,
                    backend,
                    ..
                } = subcoin_service::new_partial(&config)?;
                Ok((cmd.run(client, backend, None), task_manager))
            })
        }
        Command::Benchmark(cmd) => {
            let runner = SubstrateCli.create_runner(&*cmd)?;

            runner.sync_run(|config| {
                // This switch needs to be in the client, since the client decides
                // which sub-commands it wants to support.
                match *cmd {
                    BenchmarkCmd::Pallet(_cmd) => {
                        unimplemented!("")
                    }
                    BenchmarkCmd::Block(cmd) => {
                        let PartialComponents { client, .. } =
                            subcoin_service::new_partial(&config)?;
                        cmd.run(client)
                    }
                    #[cfg(not(feature = "runtime-benchmarks"))]
                    BenchmarkCmd::Storage(_) => Err(
                        "Storage benchmarking can be enabled with `--features runtime-benchmarks`."
                            .into(),
                    ),
                    #[cfg(feature = "runtime-benchmarks")]
                    BenchmarkCmd::Storage(cmd) => {
                        let PartialComponents {
                            client, backend, ..
                        } = subcoin_service::new_partial(&config)?;
                        let db = backend.expose_db();
                        let storage = backend.expose_storage();

                        cmd.run(config, client, db, storage)
                    }
                    BenchmarkCmd::Overhead(_cmd) => {
                        unimplemented!("")
                    }
                    BenchmarkCmd::Extrinsic(_cmd) => {
                        unimplemented!("")
                    }
                    BenchmarkCmd::Machine(cmd) => {
                        cmd.run(&config, SUBSTRATE_REFERENCE_HARDWARE.clone())
                    }
                }
            })
        }
        Command::ChainInfo(cmd) => {
            let runner = SubstrateCli.create_runner(&cmd)?;
            runner.sync_run(|config| cmd.run::<subcoin_runtime::interface::OpaqueBlock>(&config))
        }
    }
}
