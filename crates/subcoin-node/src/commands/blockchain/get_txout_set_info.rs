use super::MergedParams;
use crate::commands::blockchain::ClientParams;
use std::sync::Arc;
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
    pub(super) params: MergedParams,
}

impl GetTxOutSetInfoCmd {
    pub async fn execute(self, _client: Arc<FullClient>) -> sc_cli::Result<()> {
        // TODO: Reimplement using NativeUtxoStorage iteration.
        // Substrate UTXO storage has been removed in favor of native RocksDB storage.
        Err(sc_cli::Error::Application(Box::new(std::io::Error::other(
            "gettxoutsetinfo is not yet implemented with native UTXO storage",
        ))))
    }
}

impl From<GetTxOutSetInfo> for GetTxOutSetInfoCmd {
    fn from(get_txout_set_info: GetTxOutSetInfo) -> Self {
        let GetTxOutSetInfo { client_params, .. } = get_txout_set_info;
        Self {
            params: client_params.into_merged_params(),
        }
    }
}
