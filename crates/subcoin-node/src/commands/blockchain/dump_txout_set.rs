use super::MergedParams;
use crate::commands::blockchain::{ClientParams, fetch_utxo_set_at};
use crate::utils::Yield;
use std::fs::File;
use std::io::{Stdout, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
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

    /// Path to export the UTXO set in CSV format.
    #[clap(short, long, value_name = "PATH")]
    csv: Option<PathBuf>,

    /// Path to export the UTXO set as a binary file.
    ///
    /// The binary dump is compatible with the format used by the Bitcoin Core client.
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

        if let Some(path) = binary {
            let utxo_set_size = fetch_utxo_set_at(&client, height)?.2.count() as u64;

            println!(
                "Exporting UTXO set ({utxo_set_size}) at #{block_number},{bitcoin_block_hash} to binary file: {}",
                path.display()
            );

            let file = std::fs::File::create(&path)?;

            let mut snapshot_generator =
                UtxoSnapshotGenerator::new(path, file, bitcoin::Network::Bitcoin);

            snapshot_generator.generate_snapshot_in_mem(
                bitcoin_block_hash,
                utxo_set_size,
                utxo_iter.map(Into::into),
            )?;

            return Ok(());
        }

        let mut output_file = if let Some(path) = csv {
            println!(
                "Exporting UTXO set at #{block_number},{bitcoin_block_hash} to CSV file: {}",
                path.display()
            );
            UtxoSetOutput::Csv(std::fs::File::create(path)?)
        } else {
            println!("Exporting UTXO set at #{block_number},{bitcoin_block_hash} to stdout:");
            UtxoSetOutput::Stdout(std::io::stdout())
        };

        let processed = Arc::new(AtomicUsize::new(0));

        let grouped_utxos =
            subcoin_utxo_snapshot::group_utxos_by_txid(utxo_iter.into_iter().map(Into::into));

        let total_txids = grouped_utxos.len();

        crate::utils::show_progress_in_background(
            processed.clone(),
            total_txids as u64,
            format!("Dumping UTXO set at block #{block_number}..."),
        );

        for (txid, coins) in grouped_utxos {
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
    Csv(File),
    Stdout(Stdout),
}

impl UtxoSetOutput {
    fn write(&mut self, txid: bitcoin::Txid, vout: u32, coin: Coin) -> std::io::Result<()> {
        match self {
            Self::Csv(file) => {
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
            Self::Stdout(stdout) => {
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
