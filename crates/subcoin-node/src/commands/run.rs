use crate::cli::subcoin_params::{CommonParams, Defaults, SubcoinNetworkParams};
use clap::Parser;
use sc_cli::{
    ImportParams, NetworkParams as SubstrateNetworkParams, NodeKeyParams, PrometheusParams, Role,
    RpcEndpoint, RpcParams, SharedParams, SubstrateCli, SyncMode,
};
use sc_client_api::UsageProvider;
use sc_consensus_nakamoto::BitcoinBlockImporter;
use sc_service::config::{IpNetwork, RpcBatchRequestConfig};
use sc_service::{BasePath, Configuration, TaskManager};
use std::num::NonZeroU32;
use std::sync::Arc;
use subcoin_network::{BlockSyncOption, NetworkApi, SyncStrategy};
use subcoin_primitives::{CONFIRMATION_DEPTH, TransactionIndex};

/// Options for configuring the Subcoin network behavior.
#[derive(Copy, Default, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, clap::ValueEnum)]
pub enum SubcoinNetworkOption {
    /// Fully enables the Subcoin network, including block sync.
    #[default]
    Full,
    /// Do no run Subcoin network at all.
    None,
    /// Enables the Subcoin network with block sync disabled.
    ///
    /// This option allows block sync to occur exclusively over the Substrate network,
    /// while retaining partial Bitcoin P2P functionality (e.g., broadcasting Bitcoin transactions).
    NoBlockSync,
}

impl SubcoinNetworkOption {
    fn no_block_sync(&self) -> bool {
        matches!(self, Self::None | Self::NoBlockSync)
    }
}

/// The `run` command used to start a Subcoin node.
#[derive(Debug, Clone, Parser)]
pub struct Run {
    /// Specify the Bitcoin major sync strategy.
    ///
    /// If not provided, defaults to `SyncStrategy::BlocksFirst` in development mode and
    /// `SyncStrategy::HeadersFirst` otherwise.
    #[clap(long)]
    pub sync_strategy: Option<SyncStrategy>,

    /// Specify the target block height for the initial block download.
    ///
    /// The syncing process will stop once the best block height reaches the specified target.
    /// This flag is only used when syncing a snapshot node, which will steadily serve the state
    /// at specified block height for other node to do a state sync and UTXO snapshot download.
    #[clap(long)]
    pub sync_target: Option<u32>,

    /// Do not run the finalizer which will finalize the blocks on confirmation depth.
    #[clap(long)]
    pub no_finalizer: bool,

    /// Enable transaction indexing service.
    #[clap(long)]
    pub tx_index: bool,

    /// Specify the Subcoin network behavior.
    #[clap(long, default_value = "full")]
    pub subcoin_network: SubcoinNetworkOption,

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
    pub common_params: CommonParams,

    #[allow(missing_docs)]
    #[clap(flatten)]
    pub prometheus_params: PrometheusParams,

    #[allow(missing_docs)]
    #[clap(flatten)]
    pub rpc_params: RpcParams,

    #[allow(missing_docs)]
    #[clap(flatten)]
    pub import_params: ImportParams,

    #[allow(missing_docs)]
    #[clap(flatten)]
    pub subcoin_network_params: SubcoinNetworkParams,

    #[allow(missing_docs)]
    #[clap(flatten)]
    pub substrate_network_params: SubstrateNetworkParams,
}

fn base_path_or_default(base_path: Option<BasePath>, executable_name: &str) -> BasePath {
    base_path.unwrap_or_else(|| BasePath::from_project("", "", executable_name))
}

impl Run {
    fn subcoin_network_config(&self, network: bitcoin::Network) -> subcoin_network::Config {
        let is_dev = self.common_params.dev;

        subcoin_network::Config {
            network,
            listen_on: self.subcoin_network_params.listen,
            seednodes: self.subcoin_network_params.seednodes.clone(),
            seednode_only: self.subcoin_network_params.seednode_only,
            ipv4_only: self.subcoin_network_params.ipv4_only,
            sync_target: self.sync_target,
            max_outbound_peers: self.subcoin_network_params.max_outbound_peers,
            max_inbound_peers: self.subcoin_network_params.max_inbound_peers,
            min_sync_peer_threshold: self
                .subcoin_network_params
                .min_sync_peer_threshold
                .unwrap_or_else(|| Defaults::min_sync_peer_threshold(is_dev)),
            persistent_peer_latency_threshold: self
                .subcoin_network_params
                .persistent_peer_latency_threshold,
            sync_strategy: self
                .sync_strategy
                .unwrap_or_else(|| Defaults::sync_strategy(is_dev)),
            block_sync: self.block_sync_option(),
            base_path: base_path_or_default(
                self.common_params.base_path.clone().map(Into::into),
                &crate::substrate_cli::SubstrateCli::executable_name(),
            )
            .path()
            .to_path_buf(),
            memory_config: Default::default(),
        }
    }

    fn block_sync_option(&self) -> BlockSyncOption {
        let fast_sync_enabled = matches!(
            self.substrate_network_params.sync,
            SyncMode::Fast | SyncMode::FastUnsafe
        );

        // The block sync from bitcoin P2P network will be temporarily disabled
        // if the fast sync is enabled.
        match (self.subcoin_network.no_block_sync(), fast_sync_enabled) {
            (true, _) => BlockSyncOption::Off,
            (false, true) => BlockSyncOption::PausedUntilFastSync,
            (false, false) => BlockSyncOption::AlwaysOn,
        }
    }
}

/// Adapter of [`sc_cli::RunCmd`].
pub struct RunCmd {
    shared_params: SharedParams,
    run: Run,
}

impl RunCmd {
    pub fn new(run: Run) -> Self {
        let shared_params = run.common_params.as_shared_params();
        Self { shared_params, run }
    }

    /// Start subcoin node.
    pub async fn start(
        self,
        mut config: Configuration,
        storage_monitor: sc_storage_monitor::StorageMonitorParams,
    ) -> sc_cli::Result<TaskManager> {
        let Self {
            shared_params: _,
            run,
        } = self;

        if matches!(run.substrate_network_params.sync, SyncMode::Warp) {
            return Err(sc_cli::Error::Input(
                "--sync=warp unsupported, please use --sync=fast or --sync=fast-unsafe".to_string(),
            ));
        }

        let bitcoin_network = run.common_params.bitcoin_network();
        let import_config = run.common_params.import_config(run.common_params.dev);
        let no_finalizer = run.no_finalizer;

        let subcoin_service::NodeComponents {
            client,
            backend,
            mut task_manager,
            telemetry,
            ..
        } = subcoin_service::new_node(subcoin_service::SubcoinConfiguration {
            network: bitcoin_network,
            config: &config,
            no_hardware_benchmarks: run.no_hardware_benchmarks,
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

        let subcoin_network_config = run.subcoin_network_config(bitcoin_network);

        let network_api: Arc<dyn NetworkApi> =
            if matches!(run.subcoin_network, SubcoinNetworkOption::None) {
                task_manager.keep_alive(import_queue);
                Arc::new(subcoin_network::NoNetwork)
            } else {
                let network_handle = subcoin_network::build_network(
                    client.clone(),
                    subcoin_network_config,
                    import_queue,
                    &task_manager,
                    config.prometheus_registry().cloned(),
                    Some(substrate_sync_service.clone()),
                )
                .await
                .map_err(|err| sc_cli::Error::Application(Box::new(err)))?;

                Arc::new(network_handle)
            };

        let transaction_indexer: Arc<dyn TransactionIndex + Send + Sync> = if run.tx_index {
            let transaction_indexer = subcoin_indexer::TransactionIndexer::<
                _,
                _,
                _,
                subcoin_service::TransactionAdapter,
            >::new(
                bitcoin_network,
                client.clone(),
                task_manager.spawn_handle(),
            )
            .map_err(|err| sp_blockchain::Error::Application(Box::new(err)))?;
            spawn_handle.spawn("tx-index", None, transaction_indexer.run());
            Arc::new(subcoin_indexer::TransactionIndexProvider::new(
                client.clone(),
            ))
        } else {
            Arc::new(subcoin_primitives::NoTransactionIndex)
        };

        // Start JSON-RPC server.
        let gen_rpc_module = || {
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
                network_api.clone(),
                transaction_indexer.clone(),
            )
        };

        let rpc = sc_service::start_rpc_servers(
            &config.rpc,
            config.prometheus_registry(),
            &config.tokio_handle,
            gen_rpc_module,
            None,
        )?;
        task_manager.keep_alive((config.base_path.clone(), rpc));

        if !no_finalizer {
            spawn_handle.spawn("finalizer", None, {
                subcoin_service::SubcoinFinalizer::new(
                    client.clone(),
                    spawn_handle.clone(),
                    CONFIRMATION_DEPTH,
                    network_api.clone(),
                    Some(substrate_sync_service),
                )
                .run()
            });
        }

        // Spawn subcoin informant task.
        spawn_handle.spawn(
            "subcoin-informant",
            None,
            subcoin_informant::build(client.clone(), network_api),
        );

        Ok(task_manager)
    }
}

impl sc_cli::CliConfiguration for RunCmd {
    fn shared_params(&self) -> &SharedParams {
        &self.shared_params
    }

    fn import_params(&self) -> Option<&ImportParams> {
        Some(&self.run.import_params)
    }

    fn node_key_params(&self) -> Option<&NodeKeyParams> {
        Some(&self.run.substrate_network_params.node_key_params)
    }

    fn role(&self, _is_dev: bool) -> sc_cli::Result<Role> {
        Ok(Role::Full)
    }

    fn network_params(&self) -> Option<&SubstrateNetworkParams> {
        Some(&self.run.substrate_network_params)
    }

    fn prometheus_config(
        &self,
        default_listen_port: u16,
        chain_spec: &Box<dyn sc_service::ChainSpec>,
    ) -> sc_cli::Result<Option<sc_service::config::PrometheusConfig>> {
        Ok(self
            .run
            .prometheus_params
            .prometheus_config(default_listen_port, chain_spec.id().to_string()))
    }

    fn rpc_max_connections(&self) -> sc_cli::Result<u32> {
        Ok(self.run.rpc_params.rpc_max_connections)
    }

    fn rpc_cors(&self, is_dev: bool) -> sc_cli::Result<Option<Vec<String>>> {
        self.run.rpc_params.rpc_cors(is_dev)
    }

    fn rpc_addr(&self, default_listen_port: u16) -> sc_cli::Result<Option<Vec<RpcEndpoint>>> {
        self.run.rpc_params.rpc_addr(
            self.is_dev()?,
            false, // TODO: proper is_validator once the mining capability is added.
            default_listen_port,
        )
    }

    fn rpc_methods(&self) -> sc_cli::Result<sc_service::config::RpcMethods> {
        Ok(self.run.rpc_params.rpc_methods.into())
    }

    fn rpc_max_request_size(&self) -> sc_cli::Result<u32> {
        Ok(self.run.rpc_params.rpc_max_request_size)
    }

    fn rpc_max_response_size(&self) -> sc_cli::Result<u32> {
        Ok(self.run.rpc_params.rpc_max_response_size)
    }

    fn rpc_max_subscriptions_per_connection(&self) -> sc_cli::Result<u32> {
        Ok(self.run.rpc_params.rpc_max_subscriptions_per_connection)
    }

    fn rpc_buffer_capacity_per_connection(&self) -> sc_cli::Result<u32> {
        Ok(self
            .run
            .rpc_params
            .rpc_message_buffer_capacity_per_connection)
    }

    fn rpc_batch_config(&self) -> sc_cli::Result<RpcBatchRequestConfig> {
        self.run.rpc_params.rpc_batch_config()
    }

    fn rpc_rate_limit(&self) -> sc_cli::Result<Option<NonZeroU32>> {
        Ok(self.run.rpc_params.rpc_rate_limit)
    }

    fn rpc_rate_limit_whitelisted_ips(&self) -> sc_cli::Result<Vec<IpNetwork>> {
        Ok(self.run.rpc_params.rpc_rate_limit_whitelisted_ips.clone())
    }

    fn rpc_rate_limit_trust_proxy_headers(&self) -> sc_cli::Result<bool> {
        Ok(self.run.rpc_params.rpc_rate_limit_trust_proxy_headers)
    }
}
