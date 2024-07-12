use crate::cli::params::CommonParams;
use pallet_bitcoin::Coin;
use sc_cli::{ImportParams, NodeKeyParams, SharedParams};
use sc_client_api::{HeaderBackend, StorageProvider};
use sc_consensus_nakamoto::BlockExecutionStrategy;
use sp_core::storage::StorageKey;
use sp_core::Decode;
use std::sync::Arc;
use subcoin_primitives::{BackendExt, CoinStorageKey};
use subcoin_service::FullClient;

/// Blockchain.
#[derive(Debug, clap::Subcommand)]
pub enum Blockchain {
    /// Statistics about the unspent transaction output set.
    #[command(name = "gettxoutsetinfo")]
    GetTxOutSetInfo {
        #[clap(long)]
        height: Option<u32>,

        #[allow(missing_docs)]
        #[clap(flatten)]
        common_params: CommonParams,
    },
}

impl Blockchain {
    pub fn block_execution_strategy(&self) -> BlockExecutionStrategy {
        match self {
            Self::GetTxOutSetInfo { common_params, .. } => common_params.block_execution_strategy(),
        }
    }
}

pub enum BlockchainCmd {
    GetTxOutSetInfo {
        height: Option<u32>,
        shared_params: SharedParams,
    },
}

impl BlockchainCmd {
    /// Constructs a new instance of [`BlockchainCmd`].
    pub fn new(blockchain: Blockchain) -> Self {
        match blockchain {
            Blockchain::GetTxOutSetInfo {
                height,
                common_params,
            } => Self::GetTxOutSetInfo {
                height,
                shared_params: common_params.as_shared_params(),
            },
        }
    }

    fn shared_params(&self) -> &SharedParams {
        match self {
            Self::GetTxOutSetInfo { shared_params, .. } => shared_params,
        }
    }

    pub async fn run(self, client: Arc<FullClient>) -> sc_cli::Result<()> {
        match self {
            Self::GetTxOutSetInfo { height, .. } => gettxoutsetinfo(&client, height),
        }
    }
}

impl sc_cli::CliConfiguration for BlockchainCmd {
    fn shared_params(&self) -> &SharedParams {
        BlockchainCmd::shared_params(self)
    }

    fn import_params(&self) -> Option<&ImportParams> {
        None
    }

    fn node_key_params(&self) -> Option<&NodeKeyParams> {
        None
    }
}

fn gettxoutsetinfo(client: &Arc<FullClient>, height: Option<u32>) -> sc_cli::Result<()> {
    const FINAL_PREFIX_LEN: usize = 32;

    let storage_prefix = subcoin_service::CoinStorageKey.storage_prefix();
    let storage_key = StorageKey(storage_prefix.to_vec());
    let block_number = height.unwrap_or_else(|| client.info().best_number);
    let block_hash = client.hash(block_number)?.unwrap();
    let pairs_iter = client
        .storage_pairs(block_hash, Some(&storage_key), None)?
        .map(|(key, data)| (key.0, data.0));

    let mut txouts = 0;
    let mut bogosize = 0;
    let mut total_amount = 0;

    let genesis_txid: bitcoin::Txid =
        "4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b"
            .parse()
            .expect("Genesis txid must be correct; qed");

    for (key, value) in pairs_iter {
        let (txid, _vout) =
            <(pallet_bitcoin::Txid, u32)>::decode(&mut &key.as_slice()[FINAL_PREFIX_LEN..])
                .expect("Key type must be correct; qed");
        let txid = txid.into_bitcoin_txid();
        // output in genesis tx is excluded in gettxoutsetinfo.
        if txid == genesis_txid {
            continue;
        }
        let coin =
            Coin::decode(&mut value.as_slice()).expect("Coin must be decoded successfully; qed");
        txouts += 1;
        total_amount += coin.amount;
        bogosize += 50 + coin.script_pubkey.len();
    }

    let bitcoin_block_hash = client
        .block_hash(block_number)
        .expect("Bitcoin block hash missing");

    println!("block_number: {block_number}");
    println!("block_hash: {bitcoin_block_hash}");
    println!("txouts: {txouts}");
    println!("bogosize: {bogosize}");
    println!("total_amount: {:.8}", total_amount as f64 / 100_000_000.0);

    Ok(())
}
