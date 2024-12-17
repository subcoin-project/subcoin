use super::MergedParams;
use crate::commands::blockchain::{fetch_utxo_set_at, ClientParams};
use crate::utils::Yield;
use bitcoin::consensus::Encodable;
use std::fs::File;
use std::io::{Stdout, Write};
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use subcoin_primitives::runtime::Coin;
use subcoin_service::FullClient;
use subcoin_utxo_snapshot::UtxoSnapshotGenerator;

#[derive(Debug, clap::Parser)]
pub struct DumpTxOutSet {
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

    #[allow(missing_docs)]
    #[clap(flatten)]
    client_params: ClientParams,
}

pub struct DumpTxOutSetCmd {
    height: Option<u32>,
    binary: Option<PathBuf>,
    csv: Option<PathBuf>,
    pub(super) params: MergedParams,
}

impl DumpTxOutSetCmd {
    pub async fn execute(self, client: Arc<FullClient>) -> sc_cli::Result<()> {
        let Self {
            height,
            csv,
            binary,
            ..
        } = self;

        let (block_number, bitcoin_block_hash, utxo_iter) = fetch_utxo_set_at(&client, height)?;

        let mut output_file = if let Some(path) = csv {
            println!(
                "Dumping UTXO set at #{block_number},{bitcoin_block_hash} to {}",
                path.display()
            );
            UtxoSetOutput::Csv(std::fs::File::create(path)?)
        } else if let Some(path) = binary {
            let utxo_set_size = fetch_utxo_set_at(&client, height)?.2.count() as u64;

            println!(
                "Dumping UTXO set at #{block_number},{bitcoin_block_hash} to {}",
                path.display()
            );
            println!("UTXO set size: {utxo_set_size}");

            let file = std::fs::File::create(&path)?;

            let mut snapshot_generator =
                UtxoSnapshotGenerator::new(path, file, bitcoin::Network::Bitcoin);

            snapshot_generator.write_utxo_snapshot_in_memory(
                bitcoin_block_hash,
                utxo_set_size,
                utxo_iter.map(|(txid, vout, coin)| subcoin_utxo_snapshot::Utxo {
                    txid,
                    vout,
                    coin,
                }),
            )?;

            return Ok(());
        } else {
            println!("Dumping UTXO set at #{block_number},{bitcoin_block_hash}");
            UtxoSetOutput::Stdout(std::io::stdout())
        };

        let processed = Arc::new(AtomicUsize::new(0));

        let ordered_utxos = subcoin_utxo_snapshot::group_utxos_by_txid(
            utxo_iter
                .into_iter()
                .map(|(txid, vout, coin)| subcoin_utxo_snapshot::Utxo { txid, vout, coin }),
        );

        let total_txids = ordered_utxos.len();

        std::thread::spawn({
            let processed = processed.clone();
            move || {
                super::get_txout_set_info::show_progress(
                    processed,
                    total_txids as u64,
                    format!("Dumping UTXO set at block #{block_number}..."),
                )
            }
        });

        for (txid, coins) in ordered_utxos {
            for subcoin_utxo_snapshot::OutputEntry { vout, coin } in coins {
                output_file.write(txid, vout, coin)?;
            }
            processed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            // Yield here allows to make the process interruptible by ctrl_c.
            Yield::new().await;
        }

        Ok(())
    }
}

enum UtxoSetOutput {
    Snapshot(UtxoSnapshotGenerator),
    Csv(File),
    Stdout(Stdout),
}

impl UtxoSetOutput {
    fn write(&mut self, txid: bitcoin::Txid, vout: u32, coin: Coin) -> std::io::Result<()> {
        match self {
            Self::Snapshot(snapshot_generator) => {
                snapshot_generator.write_utxo_entry(txid, vout, coin)?;
            }
            Self::Csv(ref mut file) => {
                let Coin {
                    is_coinbase,
                    amount,
                    height,
                    script_pubkey,
                } = coin;

                let outpoint = bitcoin::OutPoint { txid, vout };

                let script_pubkey = hex::encode(script_pubkey.as_slice());
                writeln!(
                    file,
                    "{outpoint},{is_coinbase},{height},{amount},{script_pubkey}",
                )?;
            }
            Self::Stdout(ref mut stdout) => {
                let Coin {
                    is_coinbase,
                    amount,
                    height,
                    script_pubkey,
                } = coin;

                let outpoint = bitcoin::OutPoint { txid, vout };

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

impl From<DumpTxOutSet> for DumpTxOutSetCmd {
    fn from(dumptxoutset: DumpTxOutSet) -> Self {
        let DumpTxOutSet {
            height,
            csv,
            binary,
            client_params,
        } = dumptxoutset;

        Self {
            height,
            binary,
            csv,
            params: client_params.into_merged_params(),
        }
    }
}
