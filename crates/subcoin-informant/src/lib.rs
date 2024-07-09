//! # Subcoin Informant
//!
//! This crate is a fork of `sc-informant` for displaying the Subcoin network sync progress.

mod display;

use ansi_term::{Colour, Style};
use bitcoin::BlockHash;
use futures::prelude::*;
use futures_timer::Delay;
use sc_client_api::{AuxStore, BlockchainEvents, ClientInfo, UsageProvider};
use sp_blockchain::{HeaderBackend, HeaderMetadata};
use sp_runtime::traits::{Block as BlockT, Header};
use std::collections::VecDeque;
use std::fmt::Display;
use std::ops::Deref;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use subcoin_network::NetworkHandle;
use subcoin_primitives::BackendExt;
use tracing::{debug, info, trace};

/// Extended [`ClientInfo`].
#[derive(Debug)]
struct ClientInfoExt<Block: BlockT> {
    info: ClientInfo<Block>,
    best_bitcoin_hash: BlockHash,
    finalized_bitcoin_hash: BlockHash,
}

impl<Block: BlockT> Deref for ClientInfoExt<Block> {
    type Target = ClientInfo<Block>;
    fn deref(&self) -> &Self::Target {
        &self.info
    }
}

/// Creates a stream that returns a new value every `duration`.
fn interval(duration: Duration) -> impl Stream<Item = ()> + Unpin {
    futures::stream::unfold((), move |_| Delay::new(duration).map(|_| Some(((), ())))).map(drop)
}

/// The format to print telemetry output in.
#[derive(Clone, Debug)]
pub struct OutputFormat {
    /// Enable color output in logs.
    ///
    /// Is enabled by default.
    pub enable_color: bool,
}

impl Default for OutputFormat {
    fn default() -> Self {
        Self { enable_color: true }
    }
}

enum ColorOrStyle {
    Color(Colour),
    Style(Style),
}

impl From<Colour> for ColorOrStyle {
    fn from(value: Colour) -> Self {
        Self::Color(value)
    }
}

impl From<Style> for ColorOrStyle {
    fn from(value: Style) -> Self {
        Self::Style(value)
    }
}

impl ColorOrStyle {
    fn paint(&self, data: String) -> impl Display {
        match self {
            Self::Color(c) => c.paint(data),
            Self::Style(s) => s.paint(data),
        }
    }
}

impl OutputFormat {
    /// Print with color if `self.enable_color == true`.
    fn print_with_color(
        &self,
        color: impl Into<ColorOrStyle>,
        data: impl ToString,
    ) -> impl Display {
        if self.enable_color {
            color.into().paint(data.to_string()).to_string()
        } else {
            data.to_string()
        }
    }
}

/// Builds the informant and returns a `Future` that drives the informant.
pub async fn build<B: BlockT, C>(client: Arc<C>, network: NetworkHandle, format: OutputFormat)
where
    C: UsageProvider<B> + HeaderMetadata<B> + BlockchainEvents<B> + HeaderBackend<B> + AuxStore,
    <C as HeaderMetadata<B>>::Error: Display,
{
    let mut display = display::InformantDisplay::new(format.clone());

    let net_client = client.clone();

    let display_notifications = interval(Duration::from_millis(5000))
        .filter_map(|_| async { network.status().await })
        .for_each({
            move |net_status| {
                let info = net_client.usage_info();
                if let Some(ref usage) = info.usage {
                    trace!(target: "usage", "Usage statistics: {}", usage);
                } else {
                    trace!(
                        target: "usage",
                        "Usage statistics not displayed as backend does not provide it",
                    )
                }

                let best_bitcoin_hash = net_client
                    .bitcoin_block_hash_for(info.chain.best_hash)
                    .expect("Best bitcoin hash must exist; qed");

                let finalized_bitcoin_hash = net_client
                    .bitcoin_block_hash_for(info.chain.finalized_hash)
                    .expect("Finalized bitcoin hash must exist; qed");

                let client_info_ext = ClientInfoExt {
                    info,
                    best_bitcoin_hash,
                    finalized_bitcoin_hash,
                };

                display.display(client_info_ext, net_status);
                future::ready(())
            }
        });

    // TODO: proper status
    let is_major_syncing = Arc::new(true.into());

    futures::select! {
        () = display_notifications.fuse() => (),
        () = display_block_import(client, format, is_major_syncing).fuse() => (),
    };
}

fn display_block_import<B: BlockT, C>(
    client: Arc<C>,
    format: OutputFormat,
    is_major_syncing: Arc<AtomicBool>,
) -> impl Future<Output = ()>
where
    C: UsageProvider<B> + HeaderMetadata<B> + BlockchainEvents<B>,
    <C as HeaderMetadata<B>>::Error: Display,
{
    let mut last_best = {
        let info = client.usage_info();
        Some((info.chain.best_number, info.chain.best_hash))
    };

    // Hashes of the last blocks we have seen at import.
    let mut last_blocks = VecDeque::new();
    let max_blocks_to_track = 100;

    client.import_notification_stream().for_each(move |n| {
        // detect and log reorganizations.
        if let Some((ref last_num, ref last_hash)) = last_best {
            if n.header.parent_hash() != last_hash && n.is_new_best {
                let maybe_ancestor =
                    sp_blockchain::lowest_common_ancestor(&*client, *last_hash, n.hash);

                match maybe_ancestor {
                    Ok(ref ancestor) if ancestor.hash != *last_hash => info!(
                        "♻️  Reorg on #{},{} to #{},{}, common ancestor #{},{}",
                        format.print_with_color(Colour::Red.bold(), last_num),
                        last_hash,
                        format.print_with_color(Colour::Green.bold(), n.header.number()),
                        n.hash,
                        format.print_with_color(Colour::White.bold(), ancestor.number),
                        ancestor.hash,
                    ),
                    Ok(_) => {}
                    Err(e) => debug!("Error computing tree route: {}", e),
                }
            }
        }

        if n.is_new_best {
            last_best = Some((*n.header.number(), n.hash));
        }

        if is_major_syncing.load(Ordering::Relaxed) {
            return future::ready(());
        }

        // If we already printed a message for a given block recently,
        // we should not print it again.
        if !last_blocks.contains(&n.hash) {
            last_blocks.push_back(n.hash);

            if last_blocks.len() > max_blocks_to_track {
                last_blocks.pop_front();
            }

            let best_indicator = if n.is_new_best { "🏆" } else { "🆕" };
            info!(
                target: "subcoin",
                "{best_indicator} Imported #{} ({} → {})",
                format.print_with_color(Colour::White.bold(), n.header.number()),
                n.header.parent_hash(),
                n.hash,
            );
        }

        future::ready(())
    })
}
