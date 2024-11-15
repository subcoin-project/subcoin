use super::MergedParams;
use crate::commands::blockchain::{fetch_utxo_set_at, ClientParams};
use crate::utils::Yield;
use bitcoin::consensus::Encodable;
use std::fs::File;
use std::io::{Stdout, Write};
use std::path::PathBuf;
use std::sync::Arc;
use subcoin_primitives::runtime::Coin;
use subcoin_service::FullClient;
use subcoin_utxo_snapshot::UtxoSnapshotGenerator;

#[derive(Debug, clap::Parser)]
pub struct DumpTxoutSet {
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

pub struct DumpTxoutSetCommand {
    height: Option<u32>,
    binary: Option<PathBuf>,
    csv: Option<PathBuf>,
    pub params: MergedParams,
}

impl From<DumpTxoutSet> for DumpTxoutSetCommand {
    fn from(dumptxoutset: DumpTxoutSet) -> Self {
        let DumpTxoutSet {
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

impl DumpTxoutSetCommand {
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

            let mut file = std::fs::File::create(&path)?;

            let mut data = Vec::new();
            bitcoin_block_hash
                .consensus_encode(&mut data)
                .map_err(|err| sc_cli::Error::Application(Box::new(err)))?;
            utxo_set_size
                .consensus_encode(&mut data)
                .map_err(|err| sc_cli::Error::Application(Box::new(err)))?;

            let _ = file.write(data.as_slice())?;

            UtxoSetOutput::Snapshot(UtxoSnapshotGenerator::new(path, file))
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
}
