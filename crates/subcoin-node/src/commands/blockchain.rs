mod dump_txout_set;
mod get_txout_set_info;
mod parse_block_outputs;
mod parse_txout_set;

use crate::cli::subcoin_params::Chain;
use sc_cli::{DatabaseParams, ImportParams, NodeKeyParams, SharedParams};
use sc_client_api::{HeaderBackend, StorageProvider};
use sp_core::storage::StorageKey;
use sp_core::Decode;
use std::path::PathBuf;
use std::sync::Arc;
use subcoin_primitives::runtime::Coin;
use subcoin_primitives::{BackendExt, CoinStorageKey};
use subcoin_service::FullClient;

const FINAL_STORAGE_PREFIX_LEN: usize = 32;

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
    #[command(name = "parse-block-outputs")]
    ParseBlockOutputs(parse_block_outputs::ParseBlockOutputs),

    /// Parse the binary UTXO set dumped from Bitcoin Core.
    #[command(name = "parse-txout-set")]
    ParseTxoutSet(parse_txout_set::ParseTxoutSet),
}

pub enum BlockchainCmd {
    DumpTxOutSet(dump_txout_set::DumpTxOutSetCmd),
    GetTxOutSetInfo(get_txout_set_info::GetTxOutSetInfoCmd),
    ParseBlockOutputs(parse_block_outputs::ParseBlockOutputsCmd),
    ParseTxoutSet(parse_txout_set::ParseTxoutSetCmd),
}

impl BlockchainCmd {
    /// Constructs a new instance of [`BlockchainCmd`].
    pub fn new(blockchain: Blockchain) -> Self {
        match blockchain {
            Blockchain::DumpTxOutSet(cmd) => Self::DumpTxOutSet(cmd.into()),
            Blockchain::GetTxOutSetInfo(cmd) => Self::GetTxOutSetInfo(cmd.into()),
            Blockchain::ParseBlockOutputs(cmd) => Self::ParseBlockOutputs(cmd.into()),
            Blockchain::ParseTxoutSet(cmd) => Self::ParseTxoutSet(cmd.into()),
        }
    }

    pub async fn run(self, client: Arc<FullClient>) -> sc_cli::Result<()> {
        match self {
            Self::DumpTxOutSet(cmd) => cmd.execute(client).await,
            Self::GetTxOutSetInfo(cmd) => cmd.execute(client).await,
            Self::ParseBlockOutputs(cmd) => cmd.execute(client),
            Self::ParseTxoutSet(cmd) => cmd.execute(),
        }
    }
}

impl sc_cli::CliConfiguration for BlockchainCmd {
    fn shared_params(&self) -> &SharedParams {
        match self {
            Self::DumpTxOutSet(cmd) => &cmd.params.shared_params,
            Self::GetTxOutSetInfo(cmd) => &cmd.params.shared_params,
            Self::ParseBlockOutputs(cmd) => &cmd.params.shared_params,
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
            Self::ParseBlockOutputs(cmd) => Some(&cmd.params.database_params),
            Self::ParseTxoutSet(_) => None,
        }
    }

    fn node_key_params(&self) -> Option<&NodeKeyParams> {
        None
    }
}

fn fetch_utxo_set_at(
    client: &Arc<FullClient>,
    height: Option<u32>,
) -> sc_cli::Result<(
    u32,
    bitcoin::BlockHash,
    impl Iterator<Item = (bitcoin::Txid, u32, Coin)>,
)> {
    let storage_prefix = subcoin_service::CoinStorageKey.storage_prefix();
    let storage_key = StorageKey(storage_prefix.to_vec());

    let block_number = height.unwrap_or_else(|| client.info().best_number);

    let bitcoin_block_hash = client
        .block_hash(block_number)
        .ok_or(sc_cli::Error::Client(sp_blockchain::Error::Backend(
            format!("Bitcoin block hash for #{block_number} missing"),
        )))?;

    let substrate_block_hash =
        client
            .hash(block_number)?
            .ok_or(sc_cli::Error::Client(sp_blockchain::Error::Backend(
                format!("Substrate block hash for #{block_number} missing"),
            )))?;

    Ok((
        block_number,
        bitcoin_block_hash,
        client
            .storage_pairs(substrate_block_hash, Some(&storage_key), None)?
            .map(|(key, value)| {
                let (txid, vout) = <(pallet_bitcoin::types::Txid, u32)>::decode(
                    &mut &key.0.as_slice()[FINAL_STORAGE_PREFIX_LEN..],
                )
                .expect("Key type of `Coins` must be correct; qed");

                let txid = txid.into_bitcoin_txid();

                // output in genesis tx is excluded in the UTXO set.
                let coin = Coin::decode(&mut value.0.as_slice())
                    .expect("Coin read from DB must be decoded successfully; qed");

                (txid, vout, coin)
            }),
    ))
}
