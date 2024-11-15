use super::MergedParams;
use crate::commands::blockchain::{fetch_utxo_set_at, ClientParams};
use crate::utils::Yield;
use std::sync::Arc;
use std::time::{Duration, Instant};
use subcoin_service::FullClient;

#[derive(Debug, clap::Parser)]
pub struct GetTxOutSetInfo {
    /// Specify the number of block to inspect.
    ///
    /// Defaults to the best block.
    #[clap(long)]
    height: Option<u32>,

    /// Whether to display the state loading progress.
    #[clap(short, long)]
    verbose: bool,

    #[allow(missing_docs)]
    #[clap(flatten)]
    client_params: ClientParams,
}

pub struct GetTxOutSetInfoCommand {
    height: Option<u32>,
    verbose: bool,
    pub params: MergedParams,
}

impl From<GetTxOutSetInfo> for GetTxOutSetInfoCommand {
    fn from(get_txout_set_info: GetTxOutSetInfo) -> Self {
        let GetTxOutSetInfo {
            height,
            verbose,
            client_params,
        } = get_txout_set_info;
        Self {
            height,
            verbose,
            params: client_params.into_merged_params(),
        }
    }
}

impl GetTxOutSetInfoCommand {
    pub async fn execute(self, client: Arc<FullClient>) -> sc_cli::Result<()> {
        let Self {
            height, verbose, ..
        } = self;

        let (block_number, bitcoin_block_hash, utxo_iter) = fetch_utxo_set_at(&client, height)?;

        const INTERVAL: Duration = Duration::from_secs(5);

        let mut txouts = 0;
        let mut bogosize = 0;
        let mut total_amount = 0;

        let mut script_pubkey_size = 0;

        let mut last_update = Instant::now();

        let mut muhash = subcoin_crypto::muhash::MuHash3072::new();

        for (txid, vout, coin) in utxo_iter {
            let data = subcoin_utxo_snapshot::tx_out_ser(bitcoin::OutPoint { txid, vout }, &coin)
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
        let muhash = muhash.txoutset_muhash();

        let tx_out_set_info = TxOutSetInfo {
            height: block_number,
            bestblock: bitcoin_block_hash,
            txouts,
            bogosize,
            muhash,
            total_amount,
        };

        println!(
            "{}",
            serde_json::to_string_pretty(&tx_out_set_info)
                .map_err(|err| sc_cli::Error::Application(Box::new(err)))?
        );

        Ok(())
    }
}

// Custom serializer for total_amount to display 8 decimal places
fn serialize_as_btc<S>(amount: &u64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    // Convert satoshis (u64) to BTC (f64)
    let btc_value = *amount as f64 / 100_000_000.0;
    // Format the value as a string with 8 decimal places
    serializer.serialize_str(&format!("{:.8}", btc_value))
}

#[derive(serde::Serialize)]
struct TxOutSetInfo {
    height: u32,
    bestblock: bitcoin::BlockHash,
    txouts: usize,
    bogosize: usize,
    muhash: String,
    #[serde(serialize_with = "serialize_as_btc")]
    total_amount: u64,
}
