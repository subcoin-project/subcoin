use crate::cli::subcoin_params::Chain;
use crate::utils::Yield;
use bitcoin::consensus::Encodable;
use sc_cli::{DatabaseParams, ImportParams, NodeKeyParams, SharedParams};
use sc_client_api::{HeaderBackend, StorageProvider};
use sc_consensus_nakamoto::BlockExecutionStrategy;
use sp_core::storage::StorageKey;
use sp_core::Decode;
use std::fs::File;
use std::io::{Stdout, Write};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};
use subcoin_primitives::runtime::Coin;
use subcoin_primitives::{BackendExt, CoinStorageKey};
use subcoin_service::FullClient;

const FINAL_STORAGE_PREFIX_LEN: usize = 32;

static GENESIS_TXID: LazyLock<bitcoin::Txid> = LazyLock::new(|| {
    "4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b"
        .parse()
        .expect("Genesis txid must be correct; qed")
});

/// Blockchain.
#[derive(Debug, clap::Subcommand)]
pub enum Blockchain {
    /// Statistics about the UTXO set.
    #[command(name = "gettxoutsetinfo")]
    GetTxOutSetInfo {
        /// Specify the number of block to inspect.
        ///
        /// Defaults to the best block.
        #[clap(long)]
        height: Option<u32>,

        /// Whether to display the state loading progress.
        #[clap(short, long)]
        verbose: bool,

        /// Specify the chain.
        #[arg(long, value_name = "CHAIN", default_value = "bitcoin-mainnet")]
        chain: Chain,

        /// Specify custom base path.
        #[arg(long, short = 'd', value_name = "PATH")]
        base_path: Option<PathBuf>,

        #[allow(missing_docs)]
        #[clap(flatten)]
        database_params: DatabaseParams,
    },

    /// Dump UTXO set
    #[command(name = "dumptxoutset")]
    DumpTxoutSet {
        /// Specify the number of block to dump.
        ///
        /// Defaults to the best block.
        #[clap(long)]
        height: Option<u32>,

        /// Export the dumped txout set to a CSV file.
        #[clap(short, long, value_name = "PATH")]
        csv: Option<PathBuf>,

        /// Path to the binary dump file for the UTXO set.
        ///
        /// The binary dump is compatible with the format used by the Bitcoin Core client.
        /// You can export the UTXO set from Subcoin and import it into the Bitcoin
        /// Core client.
        ///
        /// If neither `--csv` nor `--binary` options are provided, the UTXO set will be
        /// printed to stdout in CSV format.
        #[clap(short, long, conflicts_with = "csv")]
        binary: Option<PathBuf>,

        /// Specify the chain.
        #[arg(long, value_name = "CHAIN", default_value = "bitcoin-mainnet")]
        chain: Chain,

        /// Specify custom base path.
        #[arg(long, short = 'd', value_name = "PATH")]
        base_path: Option<PathBuf>,

        #[allow(missing_docs)]
        #[clap(flatten)]
        database_params: DatabaseParams,
    },

    /// Parse the binary UTXO set dumped from Bitcoin Core.
    #[command(name = "parse-txout-set")]
    ParseTxoutSet {
        #[clap(short, long)]
        path: PathBuf,

        #[clap(short, long)]
        compute_addresses: bool,

        /// Specify the chain.
        #[arg(long, value_name = "CHAIN", default_value = "bitcoin-mainnet")]
        chain: Chain,
    },
}

impl Blockchain {
    pub fn block_execution_strategy(&self) -> BlockExecutionStrategy {
        BlockExecutionStrategy::runtime_disk()
    }
}

pub enum BlockchainCmd {
    GetTxOutSetInfo {
        height: Option<u32>,
        verbose: bool,
        shared_params: SharedParams,
        database_params: DatabaseParams,
    },
    DumpTxoutSet {
        height: Option<u32>,
        binary: Option<PathBuf>,
        csv: Option<PathBuf>,
        shared_params: SharedParams,
        database_params: DatabaseParams,
    },
    ParseTxoutSet {
        path: PathBuf,
        compute_addresses: bool,
        chain: Chain,
        shared_params: SharedParams,
    },
}

impl BlockchainCmd {
    /// Constructs a new instance of [`BlockchainCmd`].
    pub fn new(blockchain: Blockchain) -> Self {
        let create_shared_params = |chain: Chain, base_path| sc_cli::SharedParams {
            chain: Some(chain.chain_spec_id().to_string()),
            dev: false,
            base_path,
            log: Vec::new(),
            detailed_log_output: false,
            disable_log_color: false,
            enable_log_reloading: false,
            tracing_targets: None,
            tracing_receiver: sc_cli::TracingReceiver::Log,
        };

        match blockchain {
            Blockchain::GetTxOutSetInfo {
                height,
                verbose,
                chain,
                base_path,
                database_params,
            } => Self::GetTxOutSetInfo {
                height,
                verbose,
                shared_params: create_shared_params(chain, base_path),
                database_params,
            },
            Blockchain::DumpTxoutSet {
                height,
                csv,
                binary,
                chain,
                base_path,
                database_params,
            } => Self::DumpTxoutSet {
                height,
                binary,
                csv,
                shared_params: create_shared_params(chain, base_path),
                database_params,
            },
            Blockchain::ParseTxoutSet {
                path,
                compute_addresses,
                chain,
            } => Self::ParseTxoutSet {
                path,
                compute_addresses,
                chain,
                shared_params: create_shared_params(chain, None),
            },
        }
    }

    pub async fn run(self, client: Arc<FullClient>) -> sc_cli::Result<()> {
        match self {
            Self::GetTxOutSetInfo {
                height, verbose, ..
            } => {
                gettxoutsetinfo(&client, height, verbose).await?.print();
                Ok(())
            }
            Self::DumpTxoutSet {
                height,
                csv,
                binary,
                ..
            } => dumptxoutset(&client, height, csv, binary).await,
            Self::ParseTxoutSet {
                path,
                compute_addresses,
                chain,
                ..
            } => parse_txout_set(path, compute_addresses, chain).await,
        }
    }
}

impl sc_cli::CliConfiguration for BlockchainCmd {
    fn shared_params(&self) -> &SharedParams {
        match self {
            Self::GetTxOutSetInfo { shared_params, .. } => shared_params,
            Self::DumpTxoutSet { shared_params, .. } => shared_params,
            Self::ParseTxoutSet { shared_params, .. } => shared_params,
        }
    }

    fn import_params(&self) -> Option<&ImportParams> {
        None
    }

    fn database_params(&self) -> Option<&DatabaseParams> {
        match self {
            Self::GetTxOutSetInfo {
                database_params, ..
            } => Some(database_params),
            Self::DumpTxoutSet {
                database_params, ..
            } => Some(database_params),
            Self::ParseTxoutSet { .. } => None,
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
            .filter_map(|(key, value)| {
                let (txid, vout) = <(pallet_bitcoin::Txid, u32)>::decode(
                    &mut &key.0.as_slice()[FINAL_STORAGE_PREFIX_LEN..],
                )
                .expect("UTXO key type must be correct; qed");

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

// Equivalent function in Rust for serializing an OutPoint and Coin
//
// https://github.com/bitcoin/bitcoin/blob/6f9db1ebcab4064065ccd787161bf2b87e03cc1f/src/kernel/coinstats.cpp#L51
fn tx_out_ser(outpoint: bitcoin::OutPoint, coin: &Coin) -> bitcoin::io::Result<Vec<u8>> {
    let mut data = Vec::new();

    // Serialize the OutPoint (txid and vout)
    outpoint.consensus_encode(&mut data)?;

    // Serialize the coin's height and coinbase flag
    let height_and_coinbase = (coin.height << 1) | (coin.is_coinbase as u32);
    height_and_coinbase.consensus_encode(&mut data)?;

    let txout = bitcoin::TxOut {
        value: bitcoin::Amount::from_sat(coin.amount),
        script_pubkey: bitcoin::ScriptBuf::from_bytes(coin.script_pubkey.clone()),
    };

    // Serialize the actual UTXO (value and script)
    txout.consensus_encode(&mut data)?;

    Ok(data)
}

struct TxOutSetInfo {
    txouts: usize,
    bogosize: usize,
    muhash: String,
    total_amount: u64,
}

impl TxOutSetInfo {
    fn print(&self) {
        let Self {
            txouts,
            bogosize,
            muhash,
            total_amount,
        } = self;
        println!("====================");
        println!("txouts: {txouts}");
        println!("bogosize: {bogosize}");
        println!("muhash: {muhash}");
        println!("total_amount: {:.8}", *total_amount as f64 / 100_000_000.0);
    }
}

async fn gettxoutsetinfo(
    client: &Arc<FullClient>,
    height: Option<u32>,
    verbose: bool,
) -> sc_cli::Result<TxOutSetInfo> {
    let (block_number, bitcoin_block_hash, utxo_iter) = fetch_utxo_set_at(client, height)?;
    println!("Fetching UTXO set at block_number: #{block_number},{bitcoin_block_hash}");

    const INTERVAL: Duration = Duration::from_secs(5);

    let mut txouts = 0;
    let mut bogosize = 0;
    let mut total_amount = 0;

    let mut script_pubkey_size = 0;

    let mut last_update = Instant::now();

    let mut muhash = subcoin_crypto::muhash::MuHash3072::new();

    for (txid, vout, coin) in utxo_iter {
        let data = tx_out_ser(bitcoin::OutPoint { txid, vout }, &coin)
            .map_err(|err| sc_cli::Error::Application(Box::new(err)))?;
        muhash.insert(&data);

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

    // Hash the combined hash of all UTXOs
    let finalized = muhash.digest();

    let utxo_set_hash = finalized
        .iter()
        .rev()
        .map(|b| format!("{:02x}", b))
        .collect::<String>();

    let tx_out_set_info = TxOutSetInfo {
        txouts,
        bogosize,
        muhash: utxo_set_hash,
        total_amount,
    };

    Ok(tx_out_set_info)
}

async fn parse_txout_set(
    path: PathBuf,
    compute_addresses: bool,
    chain: Chain,
) -> sc_cli::Result<()> {
    use std::fmt::Write;

    let network = match chain {
        Chain::BitcoinSignet => bitcoin::Network::Signet,
        Chain::BitcoinTestnet => bitcoin::Network::Testnet,
        Chain::BitcoinMainnet => bitcoin::Network::Bitcoin,
    };

    let dump = txoutset::Dump::new(
        path,
        if compute_addresses {
            txoutset::ComputeAddresses::Yes(network)
        } else {
            txoutset::ComputeAddresses::No
        },
    )
    .map_err(|err| sc_cli::Error::Application(Box::new(err)))?;

    println!(
        "Dump opened.\n Block Hash: {}\n UTXO Set Size: {}",
        dump.block_hash, dump.utxo_set_size
    );

    let mut addr_str = String::new();

    for txout in dump {
        addr_str.clear();

        let txoutset::TxOut {
            address,
            amount,
            height,
            is_coinbase,
            out_point,
            script_pubkey,
        } = txout;

        match (compute_addresses, address) {
            (true, Some(address)) => {
                let _ = write!(addr_str, ",{}", address);
            }
            (true, None) => {
                let _ = write!(addr_str, ",");
            }
            (false, _) => {}
        }

        let is_coinbase = u8::from(is_coinbase);
        let amount = u64::from(amount);
        println!(
            "{out_point},{is_coinbase},{height},{amount},{}{addr_str}",
            hex::encode(script_pubkey.as_bytes()),
        );
    }

    Ok(())
}

async fn dumptxoutset(
    client: &Arc<FullClient>,
    height: Option<u32>,
    csv: Option<PathBuf>,
    binary: Option<PathBuf>,
) -> sc_cli::Result<()> {
    let (block_number, bitcoin_block_hash, utxo_iter) = fetch_utxo_set_at(client, height)?;

    let mut output_file = if let Some(path) = csv {
        println!(
            "Dumping UTXO set at #{block_number},{bitcoin_block_hash} to {}",
            path.display()
        );
        UtxoSetOutput::Csv(std::fs::File::create(path)?)
    } else if let Some(path) = binary {
        let utxo_set_size = fetch_utxo_set_at(client, height)?.2.count() as u64;

        println!(
            "Dumping UTXO set at #{block_number},{bitcoin_block_hash} to {}",
            path.display()
        );
        println!("UTXO set size: {utxo_set_size}");

        let mut file = std::fs::File::create(path)?;

        let mut data = Vec::new();
        bitcoin_block_hash
            .consensus_encode(&mut data)
            .map_err(|err| sc_cli::Error::Application(Box::new(err)))?;
        utxo_set_size
            .consensus_encode(&mut data)
            .map_err(|err| sc_cli::Error::Application(Box::new(err)))?;

        let _ = file.write(data.as_slice())?;

        UtxoSetOutput::Binary(file)
    } else {
        println!("Dumping UTXO set at #{block_number},{bitcoin_block_hash}");
        UtxoSetOutput::Stdout(std::io::stdout())
    };

    for (txid, vout, coin) in utxo_iter {
        output_file.write(txid, vout, coin)?;

        // Yield here allows to make the process interruptible by ctrl_c.
        Yield::new().await;
    }

    Ok(())
}

enum UtxoSetOutput {
    Binary(File),
    Csv(File),
    Stdout(Stdout),
}

impl UtxoSetOutput {
    fn write(&mut self, txid: bitcoin::Txid, vout: u32, coin: Coin) -> std::io::Result<()> {
        let Coin {
            is_coinbase,
            amount,
            height,
            script_pubkey,
        } = coin;

        let outpoint = bitcoin::OutPoint { txid, vout };

        match self {
            Self::Binary(ref mut file) => {
                let mut data = Vec::new();

                let amount = txoutset::Amount::new(amount);

                let code = txoutset::Code {
                    height,
                    is_coinbase,
                };
                let script = txoutset::Script::from_bytes(script_pubkey);

                outpoint.consensus_encode(&mut data)?;
                code.consensus_encode(&mut data)?;
                amount.consensus_encode(&mut data)?;
                script.consensus_encode(&mut data)?;

                let _ = file.write(data.as_slice())?;
            }
            Self::Csv(ref mut file) => {
                let script_pubkey = hex::encode(script_pubkey.as_slice());
                writeln!(
                    file,
                    "{outpoint},{is_coinbase},{height},{amount},{script_pubkey}",
                )?;
            }
            Self::Stdout(ref mut stdout) => {
                let script_pubkey = hex::encode(script_pubkey.as_slice());
                writeln!(
                    stdout,
                    "{outpoint},{is_coinbase},{height},{amount},{script_pubkey}"
                )?;
            }
        }

        Ok(())
    }
}
