use super::MergedParams;
use crate::commands::blockchain::{ClientParams, fetch_utxo_set_at};
use crate::utils::Yield;
use sc_client_api::HeaderBackend;
use sp_api::ProvideRuntimeApi;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use subcoin_primitives::runtime::SubcoinApi;
use subcoin_service::FullClient;

#[derive(Debug, clap::Parser)]
pub struct GetTxOutSetInfo {
    /// Specify the number of block to inspect.
    ///
    /// Defaults to the best block.
    #[clap(long)]
    height: Option<u32>,

    /// Disable the progress bar.
    ///
    /// The progress bar is enabled automatically when `--verbose` is false.
    #[clap(short, long)]
    no_progress: bool,

    /// Whether to display the state loading progress.
    #[clap(short, long)]
    verbose: bool,

    #[allow(missing_docs)]
    #[clap(flatten)]
    client_params: ClientParams,
}

pub struct GetTxOutSetInfoCmd {
    height: Option<u32>,
    verbose: bool,
    progress_bar: bool,
    pub(super) params: MergedParams,
}

impl GetTxOutSetInfoCmd {
    pub async fn execute(self, client: Arc<FullClient>) -> sc_cli::Result<()> {
        let Self {
            height,
            verbose,
            progress_bar,
            ..
        } = self;

        let tx_out_set_info = gettxoutsetinfo(&client, height, verbose, progress_bar).await?;

        println!(
            "{}",
            serde_json::to_string_pretty(&tx_out_set_info)
                .map_err(|err| sc_cli::Error::Application(Box::new(err)))?
        );

        Ok(())
    }
}

impl From<GetTxOutSetInfo> for GetTxOutSetInfoCmd {
    fn from(get_txout_set_info: GetTxOutSetInfo) -> Self {
        let GetTxOutSetInfo {
            height,
            verbose,
            no_progress,
            client_params,
        } = get_txout_set_info;
        Self {
            height,
            verbose,
            progress_bar: if no_progress { false } else { !verbose },
            params: client_params.into_merged_params(),
        }
    }
}

async fn gettxoutsetinfo(
    client: &Arc<FullClient>,
    height: Option<u32>,
    verbose: bool,
    progress_bar: bool,
) -> sc_cli::Result<TxOutSetInfo> {
    let (block_number, bitcoin_block_hash, utxo_iter) = fetch_utxo_set_at(client, height)?;

    let substrate_block_hash = client.hash(block_number)?.ok_or_else(|| {
        sp_blockchain::Error::Backend(format!("Hash for #{block_number} not found"))
    })?;

    let total_coins = client
        .runtime_api()
        .coins_count(substrate_block_hash)
        .map_err(|err| sc_cli::Error::from(sp_blockchain::Error::RuntimeApiError(err)))?;

    const INTERVAL: Duration = Duration::from_secs(5);

    let txouts = Arc::new(AtomicUsize::new(0));
    let mut bogosize = 0;
    let mut total_amount = 0;

    let mut script_pubkey_size = 0;

    let mut last_update = Instant::now();

    let mut muhash = subcoin_crypto::muhash::MuHash3072::new();

    if progress_bar {
        crate::utils::show_progress_in_background(
            txouts.clone(),
            total_coins,
            format!("Loading UTXO set at block #{block_number}..."),
        );
    }

    for (txid, vout, coin) in utxo_iter {
        let data = subcoin_utxo_snapshot::tx_out_ser(bitcoin::OutPoint { txid, vout }, &coin)
            .map_err(|err| sc_cli::Error::Application(Box::new(err)))?;

        muhash.insert(&data);

        let old = txouts.fetch_add(1, Ordering::SeqCst);

        total_amount += coin.amount;
        // https://github.com/bitcoin/bitcoin/blob/33af14e31b9fa436029a2bb8c2b11de8feb32f86/src/kernel/coinstats.cpp#L40
        bogosize += 50 + coin.script_pubkey.len();

        script_pubkey_size += coin.script_pubkey.len();

        if verbose && last_update.elapsed() > INTERVAL {
            println!(
                "Progress: Unspent Transaction Outputs: {}, \
                ScriptPubkey Size: {script_pubkey_size} bytes, Coin ScriptPubkey Length: {} bytes",
                old + 1,
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
        txouts: txouts.load(Ordering::SeqCst),
        bogosize,
        muhash,
        total_amount,
    };

    Ok(tx_out_set_info)
}

// Custom serializer for total_amount to display 8 decimal places
fn serialize_as_btc<S>(amount: &u64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    // Convert satoshis (u64) to BTC (f64)
    let btc_value = *amount as f64 / 100_000_000.0;
    // Format the value as a string with 8 decimal places
    serializer.serialize_str(&format!("{btc_value:.8}"))
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
            gettxoutsetinfo(&client, Some(1), false, false)
                .await
                .unwrap()
                .muhash,
            "1bd372a3f225dc6f8ce0e10ead6f8b0b00e65a2ff4a4c9ccaa615a69fbeeb2f2"
        );
        assert_eq!(
            gettxoutsetinfo(&client, Some(2), false, false)
                .await
                .unwrap()
                .muhash,
            "dfd1c34195baa0a898f04d40097841e1d569e81ce845a21e79c4ab25c725c875"
        );
    }
}
