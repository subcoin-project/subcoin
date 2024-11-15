mod dumptxoutset;
mod get_txout_set_info;
mod parse_txout_set;

use crate::cli::subcoin_params::Chain;
use dumptxoutset::DumpTxoutSetCommand;
use get_txout_set_info::GetTxOutSetInfoCommand;
use parse_txout_set::ParseTxoutSetCommand;
use sc_cli::{DatabaseParams, ImportParams, NodeKeyParams, SharedParams};
use sc_client_api::{HeaderBackend, StorageProvider};
use sp_core::storage::StorageKey;
use sp_core::Decode;
use std::path::PathBuf;
use std::sync::Arc;
use subcoin_primitives::runtime::Coin;
use subcoin_primitives::{BackendExt, BitcoinTransactionAdapter, CoinStorageKey};
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

/// Blockchain.
#[derive(Debug, clap::Subcommand)]
pub enum Blockchain {
    /// Statistics about the UTXO set.
    #[command(name = "gettxoutsetinfo")]
    GetTxOutSetInfo(get_txout_set_info::GetTxOutSetInfo),

    /// Dump UTXO set
    #[command(name = "dumptxoutset")]
    DumpTxoutSet(dumptxoutset::DumpTxoutSet),

    /// Parse the binary UTXO set dumped from Bitcoin Core.
    #[command(name = "parse-txout-set")]
    ParseTxoutSet(parse_txout_set::ParseTxoutSet),

    #[command(name = "parse-block-outputs")]
    ParseBlockOutputs {
        /// Specify the number of block to dump.
        ///
        /// Defaults to the best block.
        #[clap(long)]
        height: Option<u32>,

        #[allow(missing_docs)]
        #[clap(flatten)]
        client_params: ClientParams,
    },
}

pub struct MergedParams {
    shared_params: SharedParams,
    database_params: DatabaseParams,
}

pub enum BlockchainCommand {
    GetTxOutSetInfo(GetTxOutSetInfoCommand),
    DumpTxoutSet(DumpTxoutSetCommand),
    ParseTxoutSet(ParseTxoutSetCommand),
    ParseBlockOutputs {
        height: Option<u32>,
        params: MergedParams,
    },
}

impl BlockchainCommand {
    /// Constructs a new instance of [`BlockchainCommand`].
    pub fn new(blockchain: Blockchain) -> Self {
        match blockchain {
            Blockchain::GetTxOutSetInfo(cmd) => Self::GetTxOutSetInfo(cmd.into()),
            Blockchain::DumpTxoutSet(dumptxoutset) => Self::DumpTxoutSet(dumptxoutset.into()),
            Blockchain::ParseTxoutSet(cmd) => Self::ParseTxoutSet(cmd.into()),
            Blockchain::ParseBlockOutputs {
                height,
                client_params,
            } => Self::ParseBlockOutputs {
                height,
                params: client_params.into_merged_params(),
            },
        }
    }

    pub async fn run(self, client: Arc<FullClient>) -> sc_cli::Result<()> {
        match self {
            Self::GetTxOutSetInfo(cmd) => cmd.execute(client).await,
            Self::DumpTxoutSet(cmd) => cmd.execute(client).await,
            Self::ParseTxoutSet(cmd) => cmd.execute(),
            Self::ParseBlockOutputs { height, .. } => parse_block_outputs(&client, height),
        }
    }
}

impl sc_cli::CliConfiguration for BlockchainCommand {
    fn shared_params(&self) -> &SharedParams {
        match self {
            Self::ParseBlockOutputs { params, .. } => &params.shared_params,
            Self::GetTxOutSetInfo(cmd) => &cmd.params.shared_params,
            Self::DumpTxoutSet(cmd) => &cmd.params.shared_params,
            Self::ParseTxoutSet(cmd) => &cmd.shared_params,
        }
    }

    fn import_params(&self) -> Option<&ImportParams> {
        None
    }

    fn database_params(&self) -> Option<&DatabaseParams> {
        match self {
            Self::ParseBlockOutputs { params, .. } => Some(&params.database_params),
            Self::GetTxOutSetInfo(cmd) => Some(&cmd.params.database_params),
            Self::DumpTxoutSet(cmd) => Some(&cmd.params.database_params),
            Self::ParseTxoutSet(_) => None,
        }
    }

    fn node_key_params(&self) -> Option<&NodeKeyParams> {
        None
    }
}

fn parse_block_outputs(client: &Arc<FullClient>, height: Option<u32>) -> sc_cli::Result<()> {
    let block_number = height.unwrap_or_else(|| client.info().best_number);
    let block_hash = client.hash(block_number)?.unwrap();
    let block_body = client.body(block_hash)?.unwrap();
    let txdata = block_body
        .iter()
        .map(|xt| {
            <subcoin_service::TransactionAdapter as BitcoinTransactionAdapter<
                subcoin_runtime::interface::OpaqueBlock,
            >>::extrinsic_to_bitcoin_transaction(xt)
        })
        .collect::<Vec<_>>();
    let mut num_op_return = 0;
    for (i, tx) in txdata.into_iter().enumerate() {
        for (j, output) in tx.output.into_iter().enumerate() {
            let is_op_return = output.script_pubkey.is_op_return();
            println!("{i}:{j}: {is_op_return:?}");

            if is_op_return {
                num_op_return += 1;
            }
        }
    }
    println!("There are {num_op_return} OP_RETURN in block #{block_number}");
    Ok(())
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

                // output in genesis tx is excluded in gettxoutsetinfo and dumptxoutset in Bitcoin
                // Core.
                let coin = Coin::decode(&mut value.0.as_slice())
                    .expect("Coin read from DB must be decoded successfully; qed");

                (txid, vout, coin)
            }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use subcoin_test_service::new_test_node_and_produce_blocks;

    #[tokio::test]
    async fn test_muhash_in_gettxoutsetinfo() {
        let runtime_handle = tokio::runtime::Handle::current();
        let config = subcoin_test_service::test_configuration(runtime_handle);
        let client = new_test_node_and_produce_blocks(&config, 3).await;
        assert_eq!(
            gettxoutsetinfo(&client, Some(1), false)
                .await
                .unwrap()
                .muhash,
            "1bd372a3f225dc6f8ce0e10ead6f8b0b00e65a2ff4a4c9ccaa615a69fbeeb2f2"
        );
        assert_eq!(
            gettxoutsetinfo(&client, Some(2), false)
                .await
                .unwrap()
                .muhash,
            "dfd1c34195baa0a898f04d40097841e1d569e81ce845a21e79c4ab25c725c875"
        );
    }
}
