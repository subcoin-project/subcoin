mod dump_txout_set;
mod get_txout_set_info;
mod parse_block;
mod parse_txout_set;

use crate::cli::subcoin_params::Chain;
use sc_cli::{DatabaseParams, ImportParams, NodeKeyParams, SharedParams};
use std::path::PathBuf;
use std::sync::Arc;
use subcoin_service::FullClient;

/// Parameters for constructing a Client.
#[derive(Debug, Clone, clap::Args)]
pub struct ClientParams {
    /// Specify the chain.
    #[arg(long, value_name = "CHAIN", default_value = "bitcoin-mainnet")]
    chain: Chain,

    /// Specify custom base path.
    #[arg(long, short = 'd', value_name = "PATH")]
    base_path: Option<PathBuf>,

    #[allow(missing_docs)]
    #[clap(flatten)]
    database_params: DatabaseParams,
}

fn build_shared_params(chain: Chain, base_path: Option<PathBuf>) -> SharedParams {
    sc_cli::SharedParams {
        chain: Some(chain.chain_spec_id().to_string()),
        dev: false,
        base_path,
        log: Vec::new(),
        detailed_log_output: false,
        disable_log_color: false,
        enable_log_reloading: false,
        tracing_targets: None,
        tracing_receiver: sc_cli::TracingReceiver::Log,
    }
}

impl ClientParams {
    pub fn into_merged_params(self) -> MergedParams {
        let ClientParams {
            chain,
            base_path,
            database_params,
        } = self;

        MergedParams {
            shared_params: build_shared_params(chain, base_path),
            database_params,
        }
    }
}

pub struct MergedParams {
    shared_params: SharedParams,
    database_params: DatabaseParams,
}

/// Blockchain.
#[derive(Debug, clap::Subcommand)]
pub enum Blockchain {
    /// Dump UTXO set
    #[command(name = "dumptxoutset")]
    DumpTxOutSet(dump_txout_set::DumpTxOutSet),

    /// Statistics about the UTXO set.
    #[command(name = "gettxoutsetinfo")]
    GetTxOutSetInfo(get_txout_set_info::GetTxOutSetInfo),

    /// Inspect Bitcoin block.
    #[command(name = "parse-block")]
    ParseBlock(parse_block::ParseBlock),

    /// Parse the binary UTXO set dumped from Bitcoin Core.
    #[command(name = "parse-txout-set")]
    ParseTxoutSet(parse_txout_set::ParseTxoutSet),
}

pub enum BlockchainCmd {
    DumpTxOutSet(dump_txout_set::DumpTxOutSetCmd),
    GetTxOutSetInfo(get_txout_set_info::GetTxOutSetInfoCmd),
    ParseBlock(parse_block::ParseBlockCmd),
    ParseTxoutSet(parse_txout_set::ParseTxoutSetCmd),
}

impl BlockchainCmd {
    /// Constructs a new instance of [`BlockchainCmd`].
    pub fn new(blockchain: Blockchain) -> Self {
        match blockchain {
            Blockchain::DumpTxOutSet(cmd) => Self::DumpTxOutSet(cmd.into()),
            Blockchain::GetTxOutSetInfo(cmd) => Self::GetTxOutSetInfo(cmd.into()),
            Blockchain::ParseBlock(cmd) => Self::ParseBlock(cmd.into()),
            Blockchain::ParseTxoutSet(cmd) => Self::ParseTxoutSet(cmd.into()),
        }
    }

    pub async fn run(self, client: Arc<FullClient>) -> sc_cli::Result<()> {
        match self {
            Self::DumpTxOutSet(cmd) => cmd.execute(client).await,
            Self::GetTxOutSetInfo(cmd) => cmd.execute(client).await,
            Self::ParseBlock(cmd) => cmd.execute(client),
            Self::ParseTxoutSet(cmd) => cmd.execute(),
        }
    }
}

impl sc_cli::CliConfiguration for BlockchainCmd {
    fn shared_params(&self) -> &SharedParams {
        match self {
            Self::DumpTxOutSet(cmd) => &cmd.params.shared_params,
            Self::GetTxOutSetInfo(cmd) => &cmd.params.shared_params,
            Self::ParseBlock(cmd) => &cmd.params.shared_params,
            Self::ParseTxoutSet(cmd) => &cmd.shared_params,
        }
    }

    fn import_params(&self) -> Option<&ImportParams> {
        None
    }

    fn database_params(&self) -> Option<&DatabaseParams> {
        match self {
            Self::DumpTxOutSet(cmd) => Some(&cmd.params.database_params),
            Self::GetTxOutSetInfo(cmd) => Some(&cmd.params.database_params),
            Self::ParseBlock(cmd) => Some(&cmd.params.database_params),
            Self::ParseTxoutSet(_) => None,
        }
    }

    fn node_key_params(&self) -> Option<&NodeKeyParams> {
        None
    }
}

fn fetch_utxo_set_at(
    _client: &Arc<FullClient>,
    _height: Option<u32>,
) -> sc_cli::Result<(
    u32,
    bitcoin::BlockHash,
    std::iter::Empty<(bitcoin::Txid, u32, subcoin_primitives::runtime::Coin)>,
)> {
    // TODO: Reimplement using NativeUtxoStorage iteration.
    // Substrate UTXO storage has been removed in favor of native RocksDB storage.
    Err(sc_cli::Error::Application(Box::new(std::io::Error::other(
        "dumptxoutset is not yet implemented with native UTXO storage",
    ))))
}
