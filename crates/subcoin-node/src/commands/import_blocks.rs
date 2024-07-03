use crate::cli::params::CommonParams;
use bitcoin_explorer::BitcoinDB;
use sc_cli::{NodeKeyParams, SharedParams};
use sc_client_api::HeaderBackend;
use sc_consensus_nakamoto::{
    BitcoinBlockImport, BitcoinBlockImporter, BlockVerification, ImportConfig,
};
use sp_runtime::traits::{Block as BlockT, CheckedDiv, NumberFor, Zero};
use sp_runtime::Saturating;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use subcoin_primitives::BackendExt;
use subcoin_runtime::interface::OpaqueBlock;
use subcoin_service::FullClient;

/// Import Bitcoin blocks from bitcoind database.
#[derive(clap::Parser, Debug, Clone)]
pub struct ImportBlocks {
    /// Path of the bitcoind database.
    ///
    /// Value of the `-data-dir` argument in the bitcoind program.
    #[clap(index = 1, value_parser)]
    pub data_dir: PathBuf,

    /// Specify the block number of last block to import.
    ///
    /// The default value is the highest block in the database.
    #[clap(long)]
    pub to: Option<usize>,

    /// Whether to execute the transactions in the block.
    #[clap(long)]
    pub execute_block: bool,

    #[allow(missing_docs)]
    #[clap(flatten)]
    pub common_params: CommonParams,
}

/// Custom version of [`sc_cli::ImportBlocksCmd`].
pub struct ImportBlocksCmd {
    shared_params: SharedParams,
    to: Option<usize>,
    execute_block: bool,
}

impl ImportBlocksCmd {
    /// Constructs a new instance of [`ImportBlocksCmd`].
    pub fn new(cmd: &ImportBlocks) -> Self {
        let shared_params = cmd.common_params.as_shared_params();
        Self {
            shared_params,
            to: cmd.to,
            execute_block: cmd.execute_block,
        }
    }

    /// Run the import-blocks command
    pub async fn run(&self, client: Arc<FullClient>, data_dir: PathBuf) -> sc_cli::Result<()> {
        let from = (client.info().best_number + 1) as usize;

        let bitcoind_backend = BitcoinBackend::new(&data_dir);
        let max = bitcoind_backend.block_count();
        let to = self.to.unwrap_or(max).min(max);

        tracing::info!(
            "Start to import blocks from #{from} to #{to} from bitcoind database: {}",
            data_dir.display()
        );

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
                ImportConfig {
                    network: bitcoin::Network::Bitcoin,
                    block_verification: BlockVerification::None,
                    execute_block: self.execute_block,
                },
                Arc::new(crate::CoinStorageKey),
            );

        for index in from..=to {
            let block = bitcoind_backend.block_at(index);
            bitcoin_block_import
                .import_block(block)
                .await
                .map_err(sp_blockchain::Error::Consensus)?;

            let now = Instant::now();
            let interval_elapsed = now > last_update + INTERVAL;

            if total_imported % 1000 == 0 || interval_elapsed {
                if total_imported > 0 {
                    let info = client.info();

                    let bitcoin_block_hash =
                        BackendExt::<OpaqueBlock>::bitcoin_block_hash_for(&client, info.best_hash)
                            .unwrap_or_else(|| {
                                panic!(
                                    "bitcoin block hash for substrate#{},{} is missing",
                                    info.best_number, info.best_hash
                                )
                            });

                    let best_number = info.best_number;
                    let substrate_block_hash = info.best_hash;

                    let speed = speed::<OpaqueBlock>(best_number, last_number, last_update);

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
        }

        tracing::info!("Imported {total_imported} blocks successfully");

        Ok(())
    }
}

/// Calculates `(best_number - last_number) / (now - last_update)` and returns a `String`
/// representing the speed of import.
fn speed<B: BlockT>(
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
        format!(" {:4.1} bps", speed)
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
        format!(" {} bps", speed)
    }
}

impl sc_cli::CliConfiguration for ImportBlocksCmd {
    fn shared_params(&self) -> &SharedParams {
        &self.shared_params
    }

    fn node_key_params(&self) -> Option<&NodeKeyParams> {
        None
    }
}

struct BitcoinBackend {
    db: BitcoinDB,
}

impl BitcoinBackend {
    fn new(path: impl AsRef<Path>) -> Self {
        let db = BitcoinDB::new(path.as_ref(), true).expect("Failed to open Bitcoin DB");
        Self { db }
    }

    fn block_at(&self, height: usize) -> bitcoin::Block {
        use bitcoin::consensus::Decodable;

        let raw_block = self.db.get_raw_block(height).expect("Failed to get block");

        bitcoin::Block::consensus_decode(&mut raw_block.as_slice())
            .expect("Bad block in the database")
    }

    fn block_count(&self) -> usize {
        self.db.get_block_count()
    }
}

/// A future that will always `yield` on the first call of `poll` but schedules the
/// current task for re-execution.
///
/// This is done by getting the waker and calling `wake_by_ref` followed by returning
/// `Pending`. The next time the `poll` is called, it will return `Ready`.
struct Yield(bool);

impl Yield {
    fn new() -> Self {
        Self(false)
    }
}

impl futures::Future for Yield {
    type Output = ();

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut futures::task::Context<'_>,
    ) -> futures::task::Poll<()> {
        use futures::task::Poll;

        if !self.0 {
            self.0 = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    }
}
