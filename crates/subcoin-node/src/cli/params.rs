use clap::Parser;
use sc_consensus_nakamoto::{BlockExecutionStrategy, ExecutionBackend};
use std::path::PathBuf;
use subcoin_network::PeerId;

/// Chain.
///
/// Currently only Bitcoin is supported, more chains may be supported in the future.
#[derive(Clone, Copy, Default, Debug, clap::ValueEnum)]
pub enum Chain {
    /// Bitcoin mainnet.
    #[default]
    Bitcoin,
}

/// Bitcoin network type.
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum Network {
    /// Mainnet.
    Mainnet,
    /// Testnet.
    Testnet,
    /// Signet.
    Signet,
}

#[derive(Debug, Clone, Parser)]
pub struct NetworkParams {
    /// Specify the remote peer address to connect.
    #[clap(long, value_name = "BOOTNODE")]
    pub bootnode: Vec<String>,

    /// Connect to the nodes specified by `--bootnode` only.
    ///
    /// Do not attempt to connect to the builtin seednodes.
    #[clap(long)]
    pub bootnode_only: bool,

    /// Specify the local address and listen on it.
    #[clap(long, default_value = "127.0.0.1:8333")]
    pub listen: PeerId,

    /// Whether to connect to the nodes using IPv6 address.
    #[clap(long)]
    pub ipv4_only: bool,

    /// Specify the maximum number of inbound peers.
    #[clap(long, default_value = "100")]
    pub max_inbound_peers: usize,

    /// Specify the maximum number of outbound peers.
    #[clap(long, default_value = "10")]
    pub max_outbound_peers: usize,
}

#[derive(Debug, Clone, clap::ValueEnum)]
pub enum BlockExecution {
    /// Execute the block using runtime api `execute_block` on disk backend.
    RuntimeDisk,
    /// Execute the block using runtime api `execute_block` on in memory backend.
    RuntimeInMemory,
    /// Use custom `apply_extrinsics` on disk backend.
    OffRuntimeDisk,
    /// Use custom `apply_extrinsics` on in memory backend.
    OffRuntimeInMemory,
    /// Combo of `RuntimeDisk` and `RuntimeInMemory`.
    BenchRuntime,
    /// Benchmark all supported strategies.
    BenchAll,
}

#[derive(Debug, Clone, Parser)]
pub struct CommonParams {
    /// Specify the chain network.
    #[arg(long, value_name = "CHAIN", default_value = "bitcoin")]
    pub chain: Chain,

    /// Specify the chain network.
    #[arg(long, value_name = "NETWORK", default_value = "mainnet")]
    pub network: Network,

    /// Specify the block execution strategy.
    #[clap(long, value_enum, default_value_t = BlockExecution::RuntimeDisk)]
    pub block_execution: BlockExecution,

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
            chain: Some(format!("{:?}", self.chain)),
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

    /// Determines the Bitcoin network type based on the current chain and network settings.
    #[allow(unused)]
    pub fn bitcoin_network(&self) -> bitcoin::Network {
        match (self.chain, self.network) {
            (Chain::Bitcoin, Network::Mainnet) => bitcoin::Network::Bitcoin,
            (Chain::Bitcoin, Network::Testnet) => bitcoin::Network::Testnet,
            (Chain::Bitcoin, Network::Signet) => bitcoin::Network::Signet,
        }
    }

    pub fn block_execution_strategy(&self) -> BlockExecutionStrategy {
        match &self.block_execution {
            BlockExecution::RuntimeDisk => {
                BlockExecutionStrategy::RuntimeExecution(ExecutionBackend::Disk)
            }
            BlockExecution::RuntimeInMemory => {
                BlockExecutionStrategy::RuntimeExecution(ExecutionBackend::InMemory)
            }
            BlockExecution::OffRuntimeDisk => {
                BlockExecutionStrategy::OffRuntimeExecution(ExecutionBackend::Disk)
            }
            BlockExecution::OffRuntimeInMemory => {
                BlockExecutionStrategy::OffRuntimeExecution(ExecutionBackend::InMemory)
            }
            BlockExecution::BenchRuntime => BlockExecutionStrategy::BenchmarkRuntimeExecution,
            BlockExecution::BenchAll => BlockExecutionStrategy::BenchmarkAll,
        }
    }
}
