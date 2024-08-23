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
use sc_consensus_nakamoto::ImportConfig;
use sc_service::PartialComponents;
use std::sync::Arc;
use subcoin_primitives::CONFIRMATION_DEPTH;

#[derive(Debug, clap::Subcommand)]
pub enum Command {
    /// Run subcoin node.
    Run(Box<Run>),

    /// Import blocks from bitcoind database.
    ImportBlocks(ImportBlocks),

    /// Utility tools.
    #[command(subcommand)]
    Tools(Tools),

    /// Blockchain.
    #[command(subcommand)]
    Blockchain(Blockchain),

    /// Build a chain specification.
    BuildSpec(sc_cli::BuildSpecCmd),

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
            let run_cmd = RunCmd::new(&run);
            let runner = SubstrateCli.create_runner(&run_cmd)?;
            runner.run_node_until_exit(|config| async move {
                run_cmd
                    .start(config, *run, no_hardware_benchmarks, storage_monitor)
                    .await
            })
        }
        Command::ImportBlocks(cmd) => {
            let block_execution_strategy = cmd.common_params.block_execution_strategy();
            let bitcoin_network = cmd.common_params.bitcoin_network();
            let import_config = ImportConfig {
                execute_block: cmd.execute_transactions,
                ..cmd.common_params.import_config()
            };
            let chain_spec_id = cmd.common_params.chain.chain_spec_id();
            let maybe_prometheus_config = cmd
                .prometheus_params
                .prometheus_config(9615, chain_spec_id.to_string());
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
                    network: bitcoin_network,
                    config: &config,
                    block_execution_strategy,
                    no_hardware_benchmarks,
                    storage_monitor,
                })?;
                let spawn_handle = task_manager.spawn_handle();
                spawn_handle.spawn("finalizer", None, {
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
                        None,
                    )
                });
                Ok((
                    import_blocks_cmd.run(
                        client,
                        block_executor,
                        data_dir,
                        import_config,
                        spawn_handle,
                        maybe_prometheus_config,
                    ),
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
        Command::BuildSpec(cmd) => {
            let runner = SubstrateCli.create_runner(&cmd)?;
            runner.sync_run(|config| cmd.run(config.chain_spec, config.network))
        }
        Command::CheckBlock(cmd) => {
            let runner = SubstrateCli.create_runner(&*cmd)?;
            runner.async_run(|config| {
                let PartialComponents {
                    client,
                    task_manager,
                    import_queue,
                    ..
                } = subcoin_service::new_partial(&config, bitcoin::Network::Bitcoin)?;
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
                } = subcoin_service::new_partial(&config, bitcoin::Network::Bitcoin)?;
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
                } = subcoin_service::new_partial(&config, bitcoin::Network::Bitcoin)?;

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
                } = subcoin_service::new_partial(&config, bitcoin::Network::Bitcoin)?;
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
                            subcoin_service::new_partial(&config, bitcoin::Network::Bitcoin)?;
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
                        } = subcoin_service::new_partial(&config, bitcoin::Network::Bitcoin)?;
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
