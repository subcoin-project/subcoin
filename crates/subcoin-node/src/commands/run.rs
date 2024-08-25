use crate::cli::params::{CommonParams, NetworkParams};
use crate::cli::rpc_params::RpcParams;
use clap::Parser;
use sc_cli::{
    ImportParams, NetworkParams as SubstrateNetworkParams, NodeKeyParams, PrometheusParams, Role,
    SharedParams, SyncMode,
};
use sc_client_api::UsageProvider;
use sc_consensus_nakamoto::BitcoinBlockImporter;
use sc_service::config::{IpNetwork, RpcBatchRequestConfig};
use sc_service::{Configuration, TaskManager};
use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::sync::Arc;
use subcoin_network::SyncStrategy;
use subcoin_primitives::CONFIRMATION_DEPTH;

/// The `run` command used to run a Bitcoin node.
#[derive(Debug, Clone, Parser)]
pub struct Run {
    /// Specify the major sync strategy.
    #[clap(long, default_value = "headers-first")]
    pub sync_strategy: SyncStrategy,

    /// Do not run the finalizer which will finalize the blocks on confirmation depth.
    #[clap(long)]
    pub no_finalizer: bool,

    /// Disable the Bitcoin networking.
    #[clap(long)]
    pub disable_subcoin_networking: bool,

    #[allow(missing_docs)]
    #[clap(flatten)]
    pub rpc_params: RpcParams,

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
        let substrate_fast_sync_enabled = matches!(
            self.substrate_network_params.sync,
            SyncMode::Fast | SyncMode::FastUnsafe
        );
        subcoin_network::Params {
            network,
            listen_on: self.network_params.listen,
            seednodes: self.network_params.seednodes.clone(),
            seednode_only: self.network_params.seednode_only,
            ipv4_only: self.network_params.ipv4_only,
            max_outbound_peers: self.network_params.max_outbound_peers,
            max_inbound_peers: self.network_params.max_inbound_peers,
            sync_strategy: self.sync_strategy,
            substrate_fast_sync_enabled,
        }
    }
}

/// Adapter of [`sc_cli::RunCmd`].
pub struct RunCmd {
    rpc_params: RpcParams,
    shared_params: SharedParams,
    import_params: ImportParams,
    prometheus_params: PrometheusParams,
    substrate_network_params: SubstrateNetworkParams,
}

impl RunCmd {
    pub fn new(run: &Run) -> Self {
        let shared_params = run.common_params.as_shared_params();
        Self {
            rpc_params: run.rpc_params.clone(),
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
        let bitcoin_network = run.common_params.bitcoin_network();
        let import_config = run.common_params.import_config();
        let no_finalizer = run.no_finalizer;

        let subcoin_service::NodeComponents {
            client,
            backend,
            mut task_manager,
            block_executor,
            telemetry,
            ..
        } = subcoin_service::new_node(subcoin_service::SubcoinConfiguration {
            network: bitcoin_network,
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
                import_config,
                Arc::new(subcoin_service::CoinStorageKey),
                block_executor,
                config.prometheus_registry(),
            );

        let import_queue = sc_consensus_nakamoto::bitcoin_import_queue(
            &task_manager.spawn_essential_handle(),
            bitcoin_block_import,
        );

        let (system_rpc_tx, substrate_sync_service) = match config.network.network_backend {
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
                    bitcoin_network,
                    telemetry,
                )?
            }
            sc_network::config::NetworkBackendType::Litep2p => {
                subcoin_service::start_substrate_network::<sc_network::Litep2pNetworkBackend>(
                    &mut config,
                    client.clone(),
                    backend,
                    &mut task_manager,
                    bitcoin_network,
                    telemetry,
                )?
            }
        };

        let subcoin_network_params = run.subcoin_network_params(bitcoin_network);

        let substrate_fast_sync_enabled = subcoin_network_params.substrate_fast_sync_enabled;

        let (subcoin_networking, subcoin_network_handle) = subcoin_network::Network::new(
            client.clone(),
            subcoin_network_params,
            import_queue,
            spawn_handle.clone(),
            config.prometheus_registry().cloned(),
        );

        // TODO: handle Substrate networking and Bitcoin networking properly.
        if !run.disable_subcoin_networking {
            task_manager.spawn_essential_handle().spawn_blocking(
                "subcoin-networking",
                None,
                async move {
                    if let Err(err) = subcoin_networking.run().await {
                        tracing::error!(?err, "Error occurred in subcoin networking");
                    }
                },
            );

            if substrate_fast_sync_enabled {
                task_manager.spawn_handle().spawn(
                    "substrate-fast-sync-watcher",
                    None,
                    subcoin_service::watch_substrate_fast_sync(
                        subcoin_network_handle.clone(),
                        substrate_sync_service.clone(),
                    ),
                );
            }
        } else {
            task_manager.keep_alive(subcoin_networking);
        }

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
                subcoin_network_handle.clone(),
            )
        };

        let rpc = sc_service::start_rpc_servers(&config, gen_rpc_module, None)?;
        task_manager.keep_alive((config.base_path.clone(), rpc));

        if !no_finalizer {
            spawn_handle.spawn("finalizer", None, {
                subcoin_service::SubcoinFinalizer::new(
                    client.clone(),
                    spawn_handle.clone(),
                    CONFIRMATION_DEPTH,
                    subcoin_network_handle.is_major_syncing(),
                    Some(substrate_sync_service),
                )
                .run()
            });
        }

        // Spawn subcoin informant task.
        spawn_handle.spawn(
            "subcoin-informant",
            None,
            subcoin_informant::build(client.clone(), subcoin_network_handle),
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

    fn rpc_max_connections(&self) -> sc_cli::Result<u32> {
        Ok(self.rpc_params.rpc_max_connections)
    }

    fn rpc_cors(&self, is_dev: bool) -> sc_cli::Result<Option<Vec<String>>> {
        self.rpc_params.rpc_cors(is_dev)
    }

    fn rpc_addr(&self, default_listen_port: u16) -> sc_cli::Result<Option<SocketAddr>> {
        self.rpc_params.rpc_addr(default_listen_port)
    }

    fn rpc_methods(&self) -> sc_cli::Result<sc_service::config::RpcMethods> {
        Ok(self.rpc_params.rpc_methods.into())
    }

    fn rpc_max_request_size(&self) -> sc_cli::Result<u32> {
        Ok(self.rpc_params.rpc_max_request_size)
    }

    fn rpc_max_response_size(&self) -> sc_cli::Result<u32> {
        Ok(self.rpc_params.rpc_max_response_size)
    }

    fn rpc_max_subscriptions_per_connection(&self) -> sc_cli::Result<u32> {
        Ok(self.rpc_params.rpc_max_subscriptions_per_connection)
    }

    fn rpc_buffer_capacity_per_connection(&self) -> sc_cli::Result<u32> {
        Ok(self.rpc_params.rpc_message_buffer_capacity_per_connection)
    }

    fn rpc_batch_config(&self) -> sc_cli::Result<RpcBatchRequestConfig> {
        let cfg = if self.rpc_params.rpc_disable_batch_requests {
            RpcBatchRequestConfig::Disabled
        } else if let Some(l) = self.rpc_params.rpc_max_batch_request_len {
            RpcBatchRequestConfig::Limit(l)
        } else {
            RpcBatchRequestConfig::Unlimited
        };

        Ok(cfg)
    }

    fn rpc_rate_limit(&self) -> sc_cli::Result<Option<NonZeroU32>> {
        Ok(self.rpc_params.rpc_rate_limit)
    }

    fn rpc_rate_limit_whitelisted_ips(&self) -> sc_cli::Result<Vec<IpNetwork>> {
        Ok(self.rpc_params.rpc_rate_limit_whitelisted_ips.clone())
    }

    fn rpc_rate_limit_trust_proxy_headers(&self) -> sc_cli::Result<bool> {
        Ok(self.rpc_params.rpc_rate_limit_trust_proxy_headers)
    }
}
