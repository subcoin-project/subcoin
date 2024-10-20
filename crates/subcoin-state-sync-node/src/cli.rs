use clap::Parser;
use sc_cli::{CliConfiguration, NetworkParams, SharedParams};
use std::path::PathBuf;
use subcoin_service::ChainSpec;

const BITCOIN_MAINNET_CHAIN_SPEC: &str =
    include_str!("../../subcoin-node/res/chain-spec-raw-bitcoin-mainnet.json");

/// Fake CLI for satisfying the Substrate CLI interface.
///
/// Primarily for creating a Substrate runner.
#[derive(Debug)]
pub struct SubstrateCli;

impl sc_cli::SubstrateCli for SubstrateCli {
    fn impl_name() -> String {
        "Subcoin State Sync Node".into()
    }

    fn impl_version() -> String {
        "0.1.0".to_string()
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
            "bitcoin-mainnet" => ChainSpec::from_json_bytes(BITCOIN_MAINNET_CHAIN_SPEC.as_bytes())?,
            "bitcoin-testnet" | "bitcoin-signet" => {
                unimplemented!("Bitcoin testnet and signet are unsupported")
            }
            path => ChainSpec::from_json_file(std::path::PathBuf::from(path))?,
        };

        Ok(Box::new(chain_spec))
    }
}

/// Bitcoin chain type.
// TODO: This clippy warning will be fixed once more chains are supported.
#[allow(clippy::enum_variant_names)]
#[derive(Clone, Copy, Default, Debug, clap::ValueEnum)]
pub enum Chain {
    /// Bitcoin mainnet.
    #[default]
    BitcoinMainnet,
    /// Bitcoin testnet
    BitcoinTestnet,
    /// Bitcoin signet.
    BitcoinSignet,
}

impl Chain {
    /// Returns the value of `id` in `SubstrateCli::load_spec(id)`.
    pub fn chain_spec_id(&self) -> String {
        // Convert to kebab-case for consistency in CLI.
        match self {
            Self::BitcoinMainnet => "bitcoin-mainnet".to_string(),
            Self::BitcoinTestnet => "bitcoin-testnet".to_string(),
            Self::BitcoinSignet => "bitcoin-signet".to_string(),
        }
    }
}

/// Subcoin UTXO Set State Download Tool
#[derive(Debug, Parser)]
#[clap(version = "0.1.0")]
pub struct App {
    /// Specify the chain.
    #[arg(long, value_name = "CHAIN", default_value = "bitcoin-mainnet")]
    pub chain: Chain,

    /// Specify custom base path.
    #[arg(long, short = 'd', value_name = "PATH")]
    pub base_path: Option<PathBuf>,

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
    pub network_params: NetworkParams,
}

impl App {
    pub fn bitcoin_network(&self) -> bitcoin::Network {
        match self.chain {
            Chain::BitcoinMainnet => bitcoin::Network::Bitcoin,
            Chain::BitcoinTestnet => bitcoin::Network::Testnet,
            Chain::BitcoinSignet => bitcoin::Network::Signet,
        }
    }
}

pub struct Command {
    shared_params: SharedParams,
    network_params: NetworkParams,
}

impl Command {
    pub fn new(app: App) -> Self {
        let App {
            log,
            chain,
            base_path,
            network_params,
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
            network_params,
        }
    }
}

impl CliConfiguration for Command {
    fn shared_params(&self) -> &SharedParams {
        &self.shared_params
    }

    fn network_params(&self) -> Option<&NetworkParams> {
        Some(&self.network_params)
    }
}
