use crate::cli::params::{CommonParams, NetworkParams};
use clap::Parser;
use sc_cli::{
    ImportParams, NetworkParams as SubstrateNetworkParams, NodeKeyParams, PrometheusParams, Role,
    SharedParams,
};
use sc_client_api::UsageProvider;
use sc_consensus_nakamoto::{BitcoinBlockImporter, BlockVerification, ImportConfig};
use sc_service::{Configuration, TaskManager};
use std::sync::Arc;
use subcoin_network::SyncStrategy;
use subcoin_primitives::CONFIRMATION_DEPTH;

/// The `run` command used to run a Bitcoin node.
#[derive(Debug, Clone, Parser)]
pub struct Run {
    /// Specify the major sync strategy.
    #[clap(long, default_value = "headers-first")]
    pub sync_strategy: SyncStrategy,

    /// Specify the block verification level.
    #[clap(long, default_value = "full")]
    pub block_verification: BlockVerification,

    /// Specify the confirmation depth during the major sync.
    ///
    /// If you encounter a high memory usage when the node is major syncing, try to
    /// specify a smaller number.
    #[clap(long, default_value = "100")]
    pub major_sync_confirmation_depth: u32,

    /// Do not run the finalizer which will finalize the blocks on confirmation depth.
    #[clap(long)]
    pub no_finalizer: bool,

    /// Disable the Bitcoin networking.
    #[clap(long)]
    pub disable_subcoin_networking: bool,

    #[allow(missing_docs)]
    #[clap(flatten)]
    pub prometheus_params: PrometheusParams,

    #[allow(missing_docs)]
    #[clap(flatten)]
    pub common_params: CommonParams,

    #[allow(missing_docs)]
    #[clap(flatten)]
    pub network_params: NetworkParams,

    #[allow(missing_docs)]
    #[clap(flatten)]
    pub substrate_network_params: SubstrateNetworkParams,

    #[allow(missing_docs)]
    #[clap(flatten)]
    pub import_params: ImportParams,
}

impl Run {
    pub fn subcoin_network_params(&self, network: bitcoin::Network) -> subcoin_network::Params {
        subcoin_network::Params {
            network,
            listen_on: self.network_params.listen,
            bootnodes: self.network_params.bootnode.clone(),
            bootnode_only: self.network_params.bootnode_only,
            ipv4_only: self.network_params.ipv4_only,
            max_outbound_peers: self.network_params.max_outbound_peers,
            max_inbound_peers: self.network_params.max_inbound_peers,
            sync_strategy: self.sync_strategy,
        }
    }
}

/// Adapter of [`sc_cli::RunCmd`].
pub struct RunCmd {
    shared_params: SharedParams,
    import_params: ImportParams,
    prometheus_params: PrometheusParams,
    substrate_network_params: SubstrateNetworkParams,
}

impl RunCmd {
    pub fn new(run: &Run) -> Self {
        let shared_params = run.common_params.as_shared_params();
        Self {
            shared_params,
            prometheus_params: run.prometheus_params.clone(),
            import_params: run.import_params.clone(),
            substrate_network_params: run.substrate_network_params.clone(),
        }
    }

    /// Start subcoin node.
    pub async fn start(
        self,
        mut config: Configuration,
        run: Run,
        no_hardware_benchmarks: bool,
        storage_monitor: sc_storage_monitor::StorageMonitorParams,
    ) -> sc_cli::Result<TaskManager> {
        let block_execution_strategy = run.common_params.block_execution_strategy();
        let network = run.common_params.bitcoin_network();
        let verify_script = run.common_params.verify_script;
        let no_finalizer = run.no_finalizer;
        let major_sync_confirmation_depth = run.major_sync_confirmation_depth;

        let subcoin_service::NodeComponents {
            client,
            backend,
            mut task_manager,
            block_executor,
            keystore_container,
            telemetry,
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
                    verify_script,
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
        if !run.disable_subcoin_networking {
            task_manager.spawn_essential_handle().spawn_blocking(
                "subcoin-networking",
                None,
                async move {
                    if let Err(err) = bitcoin_network.run().await {
                        tracing::error!(?err, "Subcoin network worker exited");
                    }
                },
            );
        } else {
            task_manager.keep_alive(bitcoin_network);
        }

        let system_rpc_tx = match config.network.network_backend {
            sc_network::config::NetworkBackendType::Libp2p => {
                subcoin_service::start_substrate_network::<
                    sc_network::NetworkWorker<
                        subcoin_runtime::interface::OpaqueBlock,
                        <subcoin_runtime::interface::OpaqueBlock as sp_runtime::traits::Block>::Hash,
                    >,
                >(
                    &mut config,
                    client.clone(),
                    backend,
                    &mut task_manager,
                    keystore_container.keystore(),
                    telemetry,
                )?
            }
            sc_network::config::NetworkBackendType::Litep2p => {
                subcoin_service::start_substrate_network::<sc_network::Litep2pNetworkBackend>(
                    &mut config,
                    client.clone(),
                    backend,
                    &mut task_manager,
                    keystore_container.keystore(),
                    telemetry,
                )?
            }
        };

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
        task_manager.keep_alive((config.base_path.clone(), rpc));

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
    }
}

impl sc_cli::CliConfiguration for RunCmd {
    fn shared_params(&self) -> &SharedParams {
        &self.shared_params
    }

    fn import_params(&self) -> Option<&ImportParams> {
        Some(&self.import_params)
    }

    fn node_key_params(&self) -> Option<&NodeKeyParams> {
        Some(&self.substrate_network_params.node_key_params)
    }

    fn role(&self, _is_dev: bool) -> sc_cli::Result<Role> {
        Ok(Role::Full)
    }

    fn network_params(&self) -> Option<&SubstrateNetworkParams> {
        Some(&self.substrate_network_params)
    }

    fn prometheus_config(
        &self,
        default_listen_port: u16,
        chain_spec: &Box<dyn sc_service::ChainSpec>,
    ) -> sc_cli::Result<Option<sc_service::config::PrometheusConfig>> {
        Ok(self
            .prometheus_params
            .prometheus_config(default_listen_port, chain_spec.id().to_string()))
    }
}
