use crate::cli::subcoin_params::CommonParams;
#[cfg(feature = "remote-import")]
use crate::rpc_client::BlockstreamClient;
use crate::utils::Yield;
use bitcoin::consensus::Decodable;
use bitcoin_explorer::BitcoinDB;
use futures::FutureExt;
use sc_cli::{ImportParams, NodeKeyParams, PrometheusParams, SharedParams};
use sc_client_api::HeaderBackend;
use sc_consensus_nakamoto::{BitcoinBlockImport, BitcoinBlockImporter, ImportConfig};
use sc_service::SpawnTaskHandle;
use sc_service::config::PrometheusConfig;
use sp_consensus::BlockOrigin;
use sp_runtime::Saturating;
use sp_runtime::traits::{Block as BlockT, CheckedDiv, NumberFor, Zero};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use subcoin_primitives::BackendExt;
use subcoin_runtime::interface::OpaqueBlock;
use subcoin_service::FullClient;
use subcoin_utxo_storage::NativeUtxoStorage;

/// Custom value parser to handle both file paths and raw block data in hex format.
fn parse_raw_block(input: &str) -> Result<String, String> {
    // Try to treat the input as a file path
    let path = PathBuf::from(input);

    if path.exists() && path.is_file() {
        // If the file exists, read it as raw block data in hex.
        match std::fs::read_to_string(&path) {
            Ok(data) => Ok(data.trim().to_string()),
            Err(err) => Err(format!(
                "Failed to read block data from {}: {err}",
                path.display()
            )),
        }
    } else {
        // Otherwise, treat the input as a hex string
        Ok(input.to_string())
    }
}

/// Import Bitcoin blocks into the node.
#[derive(clap::Parser, Debug, Clone)]
pub struct ImportBlocks {
    /// Path to the bitcoind database.
    ///
    /// This corresponds to the value of the `-data-dir` argument in the bitcoind program.
    ///
    /// The blocks will be fetched from remote using the blockstream API if this argument
    /// is not specified. Note that using the remote source is only for testing purpose.
    #[clap(long, value_parser)]
    pub data_dir: Option<PathBuf>,

    /// Number of blocks to import.
    ///
    /// The process will stop after importing the specified number of blocks.
    #[clap(long)]
    pub block_count: Option<usize>,

    /// Block number of last block to import.
    ///
    /// The default value is to the highest block in the database.
    #[clap(long)]
    pub end_block: Option<usize>,

    /// Imports a single block into the node.
    ///
    /// The value can be the raw block data in hex or a path containing the block data.
    #[clap(long, value_parser = parse_raw_block, conflicts_with = "data_dir")]
    pub raw_block: Option<String>,

    /// Whether to execute the transactions within the blocks.
    #[clap(long, default_value_t = true)]
    pub execute_transactions: bool,

    /// Disable parallel script verification (for benchmarking).
    ///
    /// By default, script verification is parallelized using rayon. Use this flag
    /// to run sequential verification for performance comparison.
    #[clap(long)]
    pub no_parallel_verification: bool,

    #[allow(missing_docs)]
    #[clap(flatten)]
    pub common_params: CommonParams,

    #[allow(missing_docs)]
    #[clap(flatten)]
    pub prometheus_params: PrometheusParams,

    #[allow(missing_docs)]
    #[clap(flatten)]
    pub import_params: ImportParams,
}

/// Custom version of [`sc_cli::ImportBlocksCmd`].
pub struct ImportBlocksCmd {
    shared_params: SharedParams,
    import_params: ImportParams,
    block_count: Option<usize>,
    to: Option<usize>,
    raw_block: Option<String>,
    base_path: Option<PathBuf>,
}

impl ImportBlocksCmd {
    /// Constructs a new instance of [`ImportBlocksCmd`].
    pub fn new(cmd: &ImportBlocks) -> Self {
        let shared_params = cmd.common_params.as_shared_params();
        let import_params = cmd.import_params.clone();
        Self {
            shared_params,
            import_params,
            block_count: cmd.block_count,
            to: cmd.end_block,
            raw_block: cmd.raw_block.clone(),
            base_path: cmd.common_params.base_path.clone(),
        }
    }

    /// Create native UTXO storage (mandatory).
    fn create_native_storage(&self, chain_height: u32) -> sc_cli::Result<Arc<NativeUtxoStorage>> {
        let base_path = self
            .base_path
            .as_ref()
            .map(|p| sc_service::BasePath::new(p.clone()))
            .unwrap_or_else(|| sc_service::BasePath::from_project("", "", "subcoin"));

        let native_storage_path = base_path.path().join("native_utxo");

        tracing::info!(
            "🚀 Native UTXO storage at {}",
            native_storage_path.display()
        );

        let storage = NativeUtxoStorage::open(&native_storage_path).map_err(|err| {
            sc_cli::Error::Application(Box::new(std::io::Error::other(format!(
                "Failed to open native UTXO storage: {err:?}"
            ))))
        })?;

        // Validate native storage is in sync with chain
        let native_height = storage.height();
        if native_height != chain_height {
            return Err(sc_cli::Error::Application(Box::new(std::io::Error::other(
                format!(
                    "Native UTXO storage height ({native_height}) does not match chain height ({chain_height}). \
                    Please start fresh with a clean data directory."
                ),
            ))));
        }

        Ok(Arc::new(storage))
    }

    /// Run the import-blocks command
    pub async fn run(
        &self,
        client: Arc<FullClient>,
        maybe_data_dir: Option<PathBuf>,
        import_config: ImportConfig,
        spawn_handle: SpawnTaskHandle,
        maybe_prometheus_config: Option<PrometheusConfig>,
    ) -> sc_cli::Result<()> {
        let chain_height = client.info().best_number;
        let native_storage = self.create_native_storage(chain_height)?;

        // Import single block.
        if let Some(block_data) = &self.raw_block {
            let raw_block =
                hex::decode(block_data).map_err(|err| format!("Invalid hex data: {err}"))?;
            let block = bitcoin::Block::consensus_decode(&mut raw_block.as_slice())
                .expect("Bad block data");

            let mut bitcoin_block_import =
                BitcoinBlockImporter::<_, _, _, _, subcoin_service::TransactionAdapter>::new(
                    client.clone(),
                    client.clone(),
                    import_config,
                    native_storage,
                    maybe_prometheus_config
                        .as_ref()
                        .map(|config| config.registry.clone())
                        .as_ref(),
                );

            let block_hash = block.block_hash();

            bitcoin_block_import
                .import_block(block, BlockOrigin::Own)
                .await
                .map_err(sp_blockchain::Error::Consensus)?;

            tracing::info!("Imported block#{block_hash} successfully");

            return Ok(());
        }

        self.import_blocks(
            client,
            maybe_data_dir,
            import_config,
            spawn_handle,
            maybe_prometheus_config,
            native_storage,
        )
        .await
    }

    /// Imports a range of blocks.
    async fn import_blocks(
        &self,
        client: Arc<FullClient>,
        maybe_data_dir: Option<PathBuf>,
        import_config: ImportConfig,
        spawn_handle: SpawnTaskHandle,
        maybe_prometheus_config: Option<PrometheusConfig>,
        native_storage: Arc<NativeUtxoStorage>,
    ) -> sc_cli::Result<()> {
        let block_provider = BitcoinBlockProvider::new(maybe_data_dir)?;

        let max = block_provider.block_count().await;
        let to = self.to.unwrap_or(max).min(max);

        let from = (client.info().best_number + 1) as usize;

        tracing::info!("Start to import blocks from #{from} to #{to}",);

        const INTERVAL: Duration = Duration::from_secs(1);

        // The last time `display` or `new` has been called.
        let mut last_update = Instant::now();

        // Head of chain block number from the last time `display` has been called.
        // `None` if `display` has never been called.
        let mut last_number: Option<NumberFor<OpaqueBlock>> = None;

        let mut total_imported = 0;

        let mut bitcoin_block_import =
            BitcoinBlockImporter::<_, _, _, _, subcoin_service::TransactionAdapter>::new(
                client.clone(),
                client.clone(),
                import_config,
                native_storage,
                maybe_prometheus_config
                    .as_ref()
                    .map(|config| config.registry.clone())
                    .as_ref(),
            );

        if let Some(PrometheusConfig { port, registry }) = maybe_prometheus_config {
            spawn_handle.spawn(
                "prometheus-endpoint",
                None,
                substrate_prometheus_endpoint::init_prometheus(port, registry).map(drop),
            );
        }

        for index in from..=to {
            let block = block_provider.block_at(index).await?;
            bitcoin_block_import
                .import_block(block, BlockOrigin::Own)
                .await
                .map_err(sp_blockchain::Error::Consensus)?;

            let now = Instant::now();
            let interval_elapsed = now > last_update + INTERVAL;

            if total_imported % 1000 == 0 || interval_elapsed {
                if total_imported > 0 {
                    let info = client.info();

                    let best_number = info.best_number;
                    let substrate_block_hash = info.best_hash;

                    let bitcoin_block_hash =
                        BackendExt::<OpaqueBlock>::bitcoin_block_hash_for(&client, info.best_hash)
                            .unwrap_or_else(|| {
                                panic!(
                                    "Bitcoin block hash for substrate#{best_number},{substrate_block_hash} is missing",
                                )
                            });

                    let speed = calculate_import_speed::<OpaqueBlock>(
                        best_number,
                        last_number,
                        last_update,
                    );

                    tracing::info!(
                        "Imported {total_imported} blocks,{speed}, best#{best_number},{bitcoin_block_hash} ({substrate_block_hash})",
                    );

                    last_number.replace(best_number);
                    last_update = now;

                    // Yield here allows to make the process actually interruptible by ctrl_c.
                    Yield::new().await;
                } else if interval_elapsed {
                    tracing::info!("Imported {total_imported} blocks");
                    last_update = now;
                }
            }

            total_imported += 1;

            if let Some(block_count) = self.block_count {
                if total_imported == block_count {
                    break;
                }
            }
        }

        tracing::info!("Imported {total_imported} blocks successfully");

        Ok(())
    }
}

/// Calculates `(best_number - last_number) / (now - last_update)` and returns a `String`
/// representing the speed of import.
fn calculate_import_speed<B: BlockT>(
    best_number: NumberFor<B>,
    last_number: Option<NumberFor<B>>,
    last_update: Instant,
) -> String {
    // Number of milliseconds elapsed since last time.
    let elapsed_ms = {
        let elapsed = last_update.elapsed();
        let since_last_millis = elapsed.as_secs() * 1000;
        let since_last_subsec_millis = elapsed.subsec_millis() as u64;
        since_last_millis + since_last_subsec_millis
    };

    // Number of blocks that have been imported since last time.
    let diff = match last_number {
        None => return String::new(),
        Some(n) => best_number.saturating_sub(n),
    };

    if let Ok(diff) = TryInto::<u128>::try_into(diff) {
        // If the number of blocks can be converted to a regular integer, then it's easy: just
        // do the math and turn it into a `f64`.
        let speed = diff
            .saturating_mul(10_000)
            .checked_div(u128::from(elapsed_ms))
            .map_or(0.0, |s| s as f64)
            / 10.0;
        format!(" {speed:4.1} bps")
    } else {
        // If the number of blocks can't be converted to a regular integer, then we need a more
        // algebraic approach and we stay within the realm of integers.
        let one_thousand = NumberFor::<B>::from(1_000u32);
        let elapsed =
            NumberFor::<B>::from(<u32 as TryFrom<_>>::try_from(elapsed_ms).unwrap_or(u32::MAX));

        let speed = diff
            .saturating_mul(one_thousand)
            .checked_div(&elapsed)
            .unwrap_or_else(Zero::zero);
        format!(" {speed} bps")
    }
}

impl sc_cli::CliConfiguration for ImportBlocksCmd {
    fn shared_params(&self) -> &SharedParams {
        &self.shared_params
    }

    fn import_params(&self) -> Option<&ImportParams> {
        Some(&self.import_params)
    }

    fn node_key_params(&self) -> Option<&NodeKeyParams> {
        None
    }
}

enum BitcoinBlockProvider {
    /// Local bitcoind database.
    Local(BitcoinDB),
    /// Remote source.
    #[cfg(feature = "remote-import")]
    Remote(BlockstreamClient),
}

impl BitcoinBlockProvider {
    fn new(maybe_data_dir: Option<PathBuf>) -> sc_cli::Result<Self> {
        match maybe_data_dir {
            Some(data_dir) => {
                tracing::info!("Using local bitcoind database: {}", data_dir.display());
                let db = BitcoinDB::new(data_dir.as_ref(), true)
                    .map_err(|err| sc_cli::Error::Application(Box::new(err)))?;
                Ok(Self::Local(db))
            }
            #[cfg(feature = "remote-import")]
            None => {
                tracing::info!("Using remote block provider.");
                Ok(Self::Remote(BlockstreamClient::new()))
            }
            #[cfg(not(feature = "remote-import"))]
            None => Err(
                "Remote block import source can be enabled with `--features remote-import`".into(),
            ),
        }
    }

    async fn block_at(&self, height: usize) -> sc_cli::Result<bitcoin::Block> {
        match self {
            Self::Local(db) => {
                let raw_block = db.get_raw_block(height).map_err(|err| {
                    std::io::Error::other(format!(
                        "Failed to get bitcoin block at #{height}: {err}"
                    ))
                })?;

                Ok(bitcoin::Block::consensus_decode(&mut raw_block.as_slice())
                    .expect("Bad block in the database"))
            }
            #[cfg(feature = "remote-import")]
            Self::Remote(rpc_client) => rpc_client
                .get_block_by_height(height as u32)
                .await
                .map_err(|err| sc_cli::Error::Application(Box::new(err))),
        }
    }

    async fn block_count(&self) -> usize {
        match self {
            Self::Local(db) => db.get_block_count(),
            #[cfg(feature = "remote-import")]
            Self::Remote(rpc_client) => rpc_client
                .get_tip_height()
                .await
                .expect("Failed to fetch tip height")
                as usize,
        }
    }
}
