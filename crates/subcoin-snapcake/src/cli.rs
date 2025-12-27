use crate::params::{BitcoinChain, SnapcakeNetworkParams};
use clap::Parser;
use sc_cli::{CliConfiguration, NetworkParams, SharedParams};
use sc_service::BasePath;
use std::path::PathBuf;
use subcoin_service::ChainSpec;

const BITCOIN_MAINNET_CHAIN_SPEC: &str =
    include_str!("../../subcoin-node/res/chain-spec-raw-bitcoin-mainnet.json");

const VERSION: &str = "0.1.0";

/// Fake CLI for satisfying the Substrate CLI interface.
///
/// Primarily for creating a Substrate runner.
#[derive(Debug)]
pub struct SubstrateCli;

impl sc_cli::SubstrateCli for SubstrateCli {
    fn impl_name() -> String {
        "Subcoin Snapcake Node".into()
    }

    fn impl_version() -> String {
        VERSION.to_string()
    }

    fn description() -> String {
        env!("CARGO_PKG_DESCRIPTION").into()
    }

    fn author() -> String {
        env!("CARGO_PKG_AUTHORS").into()
    }

    fn support_url() -> String {
        "https://github.com/subcoin-project/subcoin/issues/new".into()
    }

    fn copyright_start_year() -> i32 {
        2024
    }

    fn load_spec(&self, id: &str) -> Result<Box<dyn sc_service::ChainSpec>, String> {
        let chain_spec = match id {
            "mainnet" => ChainSpec::from_json_bytes(BITCOIN_MAINNET_CHAIN_SPEC.as_bytes())?,
            "testnet" | "signet" => {
                unimplemented!("Bitcoin testnet and signet are unsupported")
            }
            path => ChainSpec::from_json_file(std::path::PathBuf::from(path))?,
        };

        Ok(Box::new(chain_spec))
    }
}

/// Subcoin Snapcake
#[derive(Debug, Parser)]
#[clap(version = VERSION)]
#[clap(about = "A decentralized tool to download UTXO set snapshot")]
pub struct App {
    /// Specify the chain.
    #[arg(long, value_name = "CHAIN", default_value = "mainnet")]
    pub chain: BitcoinChain,

    /// Specify custom base path.
    ///
    /// If this flag is not provided, a temporary directory will be created which will be
    /// deleted at the end of the process.
    #[arg(long, short = 'd', value_name = "PATH")]
    pub base_path: Option<PathBuf>,

    /// Specify the directory for generated UTXO set snapshot and other metadata files.
    #[arg(long, default_value = "./snapshots")]
    pub snapshot_dir: PathBuf,

    /// Whether to skip the state proof in state sync.
    #[arg(long, default_value = "true")]
    pub skip_proof: bool,

    /// Enable UTXO snap sync mode.
    ///
    /// When enabled, downloads UTXOs directly from peers using the P2P UTXO sync protocol
    /// instead of the legacy Substrate state sync. This is faster and more efficient.
    #[arg(long)]
    pub snap_sync: bool,

    /// Target block height for snap sync.
    ///
    /// If not specified, uses the highest available MuHash checkpoint.
    /// The specified height must have a known MuHash checkpoint.
    #[arg(long, value_name = "HEIGHT", requires = "snap_sync")]
    pub snap_sync_target: Option<u32>,

    /// Sets a custom logging filter (syntax: `<target>=<level>`).
    ///
    /// Log levels (least to most verbose) are `error`, `warn`, `info`, `debug`, and `trace`.
    ///
    /// By default, all targets log `info`. The global log level can be set with `-l<level>`.
    ///
    /// Multiple `<target>=<level>` entries can be specified and separated by a comma.
    ///
    /// *Example*: `--log error,sync=debug,grandpa=warn`.
    /// Sets Global log level to `error`, sets `sync` target to debug and grandpa target to `warn`.
    #[arg(short = 'l', long, value_name = "LOG_PATTERN", num_args = 1..)]
    pub log: Vec<String>,

    #[allow(missing_docs)]
    #[clap(flatten)]
    pub network_params: SnapcakeNetworkParams,
}

pub struct Command {
    shared_params: SharedParams,
    network_params: NetworkParams,
    snapshot_dir: PathBuf,
}

impl Command {
    /// Constructs a new instance of [`Command`].
    pub fn new(app: App) -> Self {
        let App {
            log,
            chain,
            base_path,
            network_params,
            snapshot_dir,
            ..
        } = app;

        let shared_params = SharedParams {
            chain: Some(chain.chain_spec_id()),
            dev: false,
            base_path,
            log,
            detailed_log_output: true,
            disable_log_color: false,
            enable_log_reloading: false,
            tracing_targets: None,
            tracing_receiver: sc_cli::TracingReceiver::Log,
        };

        Self {
            shared_params,
            network_params: network_params.into_network_params(),
            snapshot_dir,
        }
    }

    pub fn snapshot_dir(&self) -> PathBuf {
        self.snapshot_dir.clone()
    }
}

impl CliConfiguration<ConfigurationValues> for Command {
    fn base_path(&self) -> sc_cli::Result<Option<BasePath>> {
        Ok(self
            .shared_params()
            .base_path
            .as_ref()
            .map_or_else(|| BasePath::new_temp_dir().ok(), |p| Some(p.clone().into())))
    }

    fn shared_params(&self) -> &SharedParams {
        &self.shared_params
    }

    fn network_params(&self) -> Option<&NetworkParams> {
        Some(&self.network_params)
    }
}

/// Custom default configuration values for Subcoin State Sync Node.
pub struct ConfigurationValues;

impl sc_cli::DefaultConfigurationValues for ConfigurationValues {
    fn p2p_listen_port() -> u16 {
        30333
    }

    fn rpc_listen_port() -> u16 {
        9944
    }

    fn prometheus_listen_port() -> u16 {
        9615
    }
}
