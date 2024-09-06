use crate::cli::subcoin_params::CommonParams;
use crate::utils::Yield;
use sc_cli::{DatabaseParams, ImportParams, NodeKeyParams, SharedParams};
use sc_client_api::{HeaderBackend, StorageProvider};
use sc_consensus_nakamoto::BlockExecutionStrategy;
use sp_core::storage::StorageKey;
use sp_core::Decode;
use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::LazyLock;
use std::time::{Duration, Instant};
use subcoin_primitives::runtime::Coin;
use subcoin_primitives::{BackendExt, CoinStorageKey};
use subcoin_service::FullClient;

const FINAL_PREFIX_LEN: usize = 32;

static GENESIS_TXID: LazyLock<bitcoin::Txid> = LazyLock::new(|| {
    "4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b"
        .parse()
        .expect("Genesis txid must be correct; qed")
});

/// Blockchain.
#[derive(Debug, clap::Subcommand)]
pub enum Blockchain {
    /// Statistics about the unspent transaction output set.
    #[command(name = "gettxoutsetinfo")]
    GetTxOutSetInfo {
        #[clap(long)]
        height: Option<u32>,

        #[clap(short, long)]
        verbose: bool,

        #[allow(missing_docs)]
        #[clap(flatten)]
        common_params: CommonParams,

        #[allow(missing_docs)]
        #[clap(flatten)]
        import_params: ImportParams,
    },
    /// Dump txout set
    #[command(name = "dumptxoutset")]
    DumpTxoutSet {
        /// Specify the number of block.
        ///
        /// The default is best block.
        #[clap(long)]
        height: Option<u32>,

        /// Export the dumped txout set to a csv file.
        #[clap(short, long, value_name = "PATH")]
        csv: Option<PathBuf>,

        /// Specify the path of exported txout set.
        #[clap(short, long, conflicts_with = "csv")]
        path: Option<PathBuf>,

        #[allow(missing_docs)]
        #[clap(flatten)]
        common_params: CommonParams,

        #[allow(missing_docs)]
        #[clap(flatten)]
        database_params: DatabaseParams,
    },
}

impl Blockchain {
    pub fn block_execution_strategy(&self) -> BlockExecutionStrategy {
        match self {
            Self::GetTxOutSetInfo { common_params, .. } => common_params.block_execution_strategy(),
            Self::DumpTxoutSet { common_params, .. } => common_params.block_execution_strategy(),
        }
    }
}

pub enum BlockchainCmd {
    GetTxOutSetInfo {
        height: Option<u32>,
        shared_params: SharedParams,
        import_params: ImportParams,
        verbose: bool,
    },
    DumpTxoutSet {
        height: Option<u32>,
        path: Option<PathBuf>,
        csv: Option<PathBuf>,
        shared_params: SharedParams,
        database_params: DatabaseParams,
    },
}

impl BlockchainCmd {
    /// Constructs a new instance of [`BlockchainCmd`].
    pub fn new(blockchain: Blockchain) -> Self {
        match blockchain {
            Blockchain::GetTxOutSetInfo {
                height,
                common_params,
                import_params,
                verbose,
            } => Self::GetTxOutSetInfo {
                height,
                shared_params: common_params.as_shared_params(),
                import_params,
                verbose,
            },
            Blockchain::DumpTxoutSet {
                height,
                csv,
                path,
                common_params,
                database_params,
            } => Self::DumpTxoutSet {
                height,
                path,
                csv,
                shared_params: common_params.as_shared_params(),
                database_params,
            },
        }
    }

    pub async fn run(self, client: Arc<FullClient>) -> sc_cli::Result<()> {
        match self {
            Self::GetTxOutSetInfo {
                height, verbose, ..
            } => gettxoutsetinfo(&client, height, verbose).await,
            Self::DumpTxoutSet {
                height, csv, path, ..
            } => dumptxoutset(&client, height, path, csv).await,
        }
    }
}

impl sc_cli::CliConfiguration for BlockchainCmd {
    fn shared_params(&self) -> &SharedParams {
        match self {
            Self::GetTxOutSetInfo { shared_params, .. } => shared_params,
            Self::DumpTxoutSet { shared_params, .. } => shared_params,
        }
    }

    fn import_params(&self) -> Option<&ImportParams> {
        match self {
            Self::GetTxOutSetInfo { import_params, .. } => Some(import_params),
            Self::DumpTxoutSet { .. } => None,
        }
    }

    fn database_params(&self) -> Option<&DatabaseParams> {
        match self {
            Self::GetTxOutSetInfo { .. } => None,
            Self::DumpTxoutSet {
                database_params, ..
            } => Some(database_params),
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
        .expect("Bitcoin block hash missing");

    let substrate_block_hash = client
        .hash(block_number)?
        .expect("Substrate block hash missing");

    Ok((
        block_number,
        bitcoin_block_hash,
        client
            .storage_pairs(substrate_block_hash, Some(&storage_key), None)?
            .filter_map(|(key, value)| {
                let (txid, vout) = <(pallet_bitcoin::Txid, u32)>::decode(
                    &mut &key.0.as_slice()[FINAL_PREFIX_LEN..],
                )
                .expect("Key type must be correct; qed");

                let txid = txid.into_bitcoin_txid();

                // output in genesis tx is excluded in gettxoutsetinfo and dumptxoutset in Bitcoin
                // Core.
                if txid == *GENESIS_TXID {
                    None
                } else {
                    let coin = Coin::decode(&mut value.0.as_slice())
                        .expect("Coin read from DB must be decoded successfully; qed");

                    Some((txid, vout, coin))
                }
            }),
    ))
}

async fn gettxoutsetinfo(
    client: &Arc<FullClient>,
    height: Option<u32>,
    verbose: bool,
) -> sc_cli::Result<()> {
    let (block_number, bitcoin_block_hash, utxo_iter) = fetch_utxo_set_at(client, height)?;

    const INTERVAL: Duration = Duration::from_secs(5);

    println!("Fetching state info at block_number: #{block_number},{bitcoin_block_hash}");

    let mut txouts = 0;
    let mut bogosize = 0;
    let mut total_amount = 0;

    let mut script_pubkey_size = 0;

    let mut last_update = Instant::now();

    for (_txid, _vout, coin) in utxo_iter {
        txouts += 1;
        total_amount += coin.amount;
        // https://github.com/bitcoin/bitcoin/blob/33af14e31b9fa436029a2bb8c2b11de8feb32f86/src/kernel/coinstats.cpp#L40
        bogosize += 50 + coin.script_pubkey.len();

        script_pubkey_size += coin.script_pubkey.len();

        if verbose && last_update.elapsed() > INTERVAL {
            println!(
                "Progress: Unspent Transaction Outputs: {txouts}, \
                ScriptPubkey Size: {script_pubkey_size} bytes, Coin ScriptPubkey Length: {} bytes",
                coin.script_pubkey.len()
            );
            last_update = Instant::now();
        }

        // Yield here allows to make the process interruptible by ctrl_c.
        Yield::new().await;
    }

    println!("====================");
    println!("txouts: {txouts}");
    println!("bogosize: {bogosize}");
    println!("total_amount: {:.8}", total_amount as f64 / 100_000_000.0);
    println!("script_pubkey_size: {script_pubkey_size} bytes");

    Ok(())
}

enum OutputFile {
    Binary(File),
    Csv(File),
}

impl OutputFile {
    fn new(path: PathBuf, csv: bool) -> std::io::Result<Self> {
        let file = std::fs::File::create(path)?;
        if csv {
            Ok(Self::Csv(file))
        } else {
            Ok(Self::Binary(file))
        }
    }

    fn write(&mut self, txid: bitcoin::Txid, vout: u32, coin: Coin) -> std::io::Result<()> {
        use bitcoin::consensus::encode::Encodable;
        use std::io::Write;

        let Coin {
            is_coinbase,
            amount,
            height,
            script_pubkey,
        } = coin;

        let outpoint = bitcoin::OutPoint { txid, vout };

        match self {
            Self::Csv(mut file) => {
                let is_coinbase = if is_coinbase { 1u8 } else { 0u8 };
                let script_pubkey = hex::encode(script_pubkey.as_slice());
                writeln!(
                    file,
                    "{outpoint},{is_coinbase},{height},{amount},{script_pubkey}",
                )?;
            }
            Self::Binary(mut file) => {
                let mut data = Vec::new();

                let amount = txoutset::Amount::new(amount);
                let code = txoutset::Code {
                    height,
                    is_coinbase,
                };
                let script = txoutset::Script::from_bytes(script_pubkey);

                outpoint.consensus_encode(&mut data)?;
                code.consensus_encode(&mut data)?;
                script.consensus_encode(&mut data)?;

                file.write(data.as_slice())?;
            }
        }

        Ok(())
    }
}

async fn dumptxoutset(
    client: &Arc<FullClient>,
    height: Option<u32>,
    path: Option<PathBuf>,
    csv: Option<PathBuf>,
) -> sc_cli::Result<()> {
    use std::io::Write;

    let (block_number, bitcoin_block_hash, utxo_iter) = fetch_utxo_set_at(client, height)?;

    println!("Fetching UTXO set at block_number: #{block_number},{bitcoin_block_hash}");

    let mut output_file = if let Some(path) = path {
        println!("Export UTXO set at {}", path.display());
        Some(std::fs::File::create(path)?)
    } else if let Some(path) = csv {
        println!("Export UTXO set to csv file at {}", path.display());
        Some(std::fs::File::create(path)?)
    } else {
        None
    };

    for (txid, vout, coin) in utxo_iter {
        let Coin {
            is_coinbase,
            amount,
            height,
            script_pubkey,
        } = coin;

        let is_coinbase = if is_coinbase { 1u8 } else { 0u8 };
        let outpoint = bitcoin::OutPoint { txid, vout };
        let script_pubkey = hex::encode(script_pubkey.as_slice());

        if let Some(ref mut output_file) = output_file {
            writeln!(
                output_file,
                "{outpoint},{is_coinbase},{height},{amount},{script_pubkey}",
            )?;
        } else {
            println!("{outpoint},{is_coinbase},{height},{amount},{script_pubkey}");
        }

        // Yield here allows to make the process interruptible by ctrl_c.
        Yield::new().await;
    }

    Ok(())
}
