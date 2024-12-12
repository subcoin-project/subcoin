use clap::Parser;
use sc_consensus_nakamoto::{BlockVerification, ImportConfig};
use std::path::PathBuf;
use subcoin_network::PeerId;

/// Chain.
///
/// Currently only Bitcoin is supported, more chains may be supported in the future.
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
    pub fn chain_spec_id(&self) -> &'static str {
        // Convert to kebab-case for consistency in CLI.
        match self {
            Self::BitcoinMainnet => "bitcoin-mainnet",
            Self::BitcoinTestnet => "bitcoin-testnet",
            Self::BitcoinSignet => "bitcoin-signet",
        }
    }
}

/// Subcoin networking params.
#[derive(Debug, Clone, Parser)]
pub struct NetworkParams {
    /// Specify the remote peer address to connect.
    #[clap(long, value_name = "SEEDNODE")]
    pub seednodes: Vec<String>,

    /// Connect to the nodes specified by `--seednode` only.
    ///
    /// Do not attempt to connect to the builtin seednodes.
    #[clap(long)]
    pub seednode_only: bool,

    /// Specify the local address and listen on it.
    #[clap(long, default_value = "127.0.0.1:8333")]
    pub listen: PeerId,

    /// Whether to connect to the nodes using IPv6 address.
    #[clap(long)]
    pub ipv4_only: bool,

    /// Specify the maximum number of inbound subcoin networking peers.
    #[clap(long, default_value = "100")]
    pub max_inbound_peers: usize,

    /// Specify the maximum number of outbound subcoin networking peers.
    #[clap(long, default_value = "20")]
    pub max_outbound_peers: usize,

    /// Persistent peer latency threshold in milliseconds (ms).
    ///
    /// Only peers with a latency lower than this threshold can possibly be saved to disk.
    ///
    /// Default value is 500 ms
    #[clap(long, default_value_t = 500)]
    pub persistent_peer_latency_threshold: u128,

    /// Minimum peer threshold required to start block sync.
    ///
    /// The chain sync won't be started until the number of sync peers reaches this threshold.
    /// Set to `0` to disable the peer threshold limit. Default: 3
    #[arg(long, default_value = "3")]
    pub min_peer_threshold: usize,
}

#[derive(Debug, Clone, Parser)]
pub struct CommonParams {
    /// Specify the chain.
    #[arg(long, value_name = "CHAIN", default_value = "bitcoin-mainnet")]
    pub chain: Chain,

    /// Specify the block verification level.
    #[clap(long, default_value = "full")]
    pub block_verification: BlockVerification,

    /// Whether to verify the TxIn scripts during the block verification.
    #[clap(long, default_value_t = true)]
    pub verify_script: bool,

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

    /// Sets a custom profiling filter.
    ///
    /// Syntax is the same as for logging (`--log`).
    #[arg(long, value_name = "TARGETS")]
    pub tracing_targets: Option<String>,
}

impl CommonParams {
    /// Converts the current [`CommonParams`] to a [`sc_cli::SharedParams`].
    pub fn as_shared_params(&self) -> sc_cli::SharedParams {
        // TODO: expose more flags?
        sc_cli::SharedParams {
            chain: Some(self.chain.chain_spec_id().to_string()),
            dev: false,
            base_path: self.base_path.clone(),
            log: self.log.clone(),
            detailed_log_output: false,
            disable_log_color: false,
            enable_log_reloading: false,
            tracing_targets: self.tracing_targets.clone(),
            tracing_receiver: sc_cli::TracingReceiver::Log,
        }
    }

    /// Determines the Bitcoin network type based on the current chain setting.
    pub fn bitcoin_network(&self) -> bitcoin::Network {
        match self.chain {
            Chain::BitcoinMainnet => bitcoin::Network::Bitcoin,
            Chain::BitcoinTestnet => bitcoin::Network::Testnet,
            Chain::BitcoinSignet => bitcoin::Network::Signet,
        }
    }

    pub fn import_config(&self) -> ImportConfig {
        ImportConfig {
            network: self.bitcoin_network(),
            block_verification: self.block_verification,
            execute_block: true,
            verify_script: self.verify_script,
        }
    }
}
