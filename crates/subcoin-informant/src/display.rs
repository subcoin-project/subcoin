// This file is part of Substrate.

// Copyright (C) Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: GPL-3.0-or-later WITH Classpath-exception-2.0

// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.

use crate::ClientInfoExt;
use bitcoin::hashes::Hash;
use bitcoin::BlockHash;
use console::style;
use sp_runtime::traits::{Block as BlockT, CheckedDiv, NumberFor, Saturating, Zero};
use sp_runtime::SaturatedConversion;
use std::fmt::{self, Display};
use std::time::Instant;
use subcoin_network::{NetworkStatus, SyncStatus};

struct DisplayBlockHash(BlockHash);

impl Display for DisplayBlockHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        const BYTES: usize = 32;
        let bytes = self.0.as_byte_array();
        for i in bytes[BYTES - 2..BYTES].iter().rev() {
            write!(f, "{:02x}", i)?;
        }
        write!(f, "…")?;
        for i in bytes[0..3].iter().rev() {
            write!(f, "{:02x}", i)?;
        }
        Ok(())
    }
}

/// State of the informant display system.
///
/// This is the system that handles the line that gets regularly printed and that looks something
/// like:
///
/// > Syncing  5.4 bps, target=#531028 (4 peers), best: #90683 (0x4ca8…51b8),
/// > finalized #360 (0x6f24…a38b), ⬇ 5.5kiB/s ⬆ 0.9kiB/s
///
/// # Usage
///
/// Call `InformantDisplay::new` to initialize the state, then regularly call `display` with the
/// information to display.
pub struct InformantDisplay<B: BlockT> {
    /// Head of chain block number from the last time `display` has been called.
    /// `None` if `display` has never been called.
    last_number: Option<NumberFor<B>>,
    /// The last time `display` or `new` has been called.
    last_update: Instant,
    /// The last seen total of bytes received.
    last_total_bytes_inbound: u64,
    /// The last seen total of bytes sent.
    last_total_bytes_outbound: u64,
}

impl<B: BlockT> InformantDisplay<B> {
    /// Builds a new informant display system.
    pub fn new() -> InformantDisplay<B> {
        InformantDisplay {
            last_number: None,
            last_update: Instant::now(),
            last_total_bytes_inbound: 0,
            last_total_bytes_outbound: 0,
        }
    }

    /// Displays the informant by calling `info!`.
    pub fn display(&mut self, info: ClientInfoExt<B>, net_status: NetworkStatus) {
        let best_number = info.chain.best_number;
        let best_hash = info.chain.best_hash;
        let best_bitcoin_hash = DisplayBlockHash(info.best_bitcoin_hash);
        let finalized_number = info.chain.finalized_number;
        let finalized_bitcoin_hash = DisplayBlockHash(info.finalized_bitcoin_hash);
        let speed = speed::<B>(best_number, self.last_number, self.last_update);
        let num_connected_peers = net_status.num_connected_peers;
        let total_bytes_inbound = net_status.total_bytes_inbound;
        let total_bytes_outbound = net_status.total_bytes_outbound;

        let now = Instant::now();
        let elapsed = (now - self.last_update).as_secs();
        self.last_update = now;
        self.last_number = Some(best_number);

        let diff_bytes_inbound = total_bytes_inbound - self.last_total_bytes_inbound;
        let diff_bytes_outbound = total_bytes_outbound - self.last_total_bytes_outbound;
        let (avg_bytes_per_sec_inbound, avg_bytes_per_sec_outbound) = if elapsed > 0 {
            self.last_total_bytes_inbound = total_bytes_inbound;
            self.last_total_bytes_outbound = total_bytes_outbound;
            (diff_bytes_inbound / elapsed, diff_bytes_outbound / elapsed)
        } else {
            (diff_bytes_inbound, diff_bytes_outbound)
        };

        let (level, status, target) = match net_status.sync_status {
            SyncStatus::Idle => ("💤", "Idle".into(), "".into()),
            SyncStatus::Downloading { target, .. } => {
                let progress = best_number.saturated_into::<u32>() as f64 * 100.0 / target as f64;
                (
                    "⚙️ ",
                    format!("Syncing{speed}"),
                    format!(", target=#{target} ({progress:.2}%)"),
                )
            }
            SyncStatus::Importing { target, .. } => {
                let progress = best_number.saturated_into::<u32>() as f64 * 100.0 / target as f64;
                (
                    "⚙️ ",
                    format!("Preparing{speed}"),
                    format!(", target=#{target} ({progress:.2}%)"),
                )
            }
        };

        let finalized_hash = info.chain.finalized_hash;

        tracing::info!(
            target: "subcoin",
            "{level} {}{target} ({} peers), best: #{} ({best_bitcoin_hash},{best_hash}), finalized #{} ({finalized_bitcoin_hash},{finalized_hash}), ⬇ {} ⬆ {}",
            style(status).white().bold(),
            style(num_connected_peers).white().bold(),
            style(best_number).white().bold(),
            style(finalized_number).white().bold(),
            style(TransferRateFormat(avg_bytes_per_sec_inbound)).green(),
            style(TransferRateFormat(avg_bytes_per_sec_outbound)).red(),
        )
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

/// Contains a number of bytes per second. Implements `fmt::Display` and shows this number of bytes
/// per second in a nice way.
struct TransferRateFormat(u64);
impl fmt::Display for TransferRateFormat {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // Special case 0.
        if self.0 == 0 {
            return write!(f, "0");
        }

        // Under 0.1 kiB, display plain bytes.
        if self.0 < 100 {
            return write!(f, "{} B/s", self.0);
        }

        // Under 1.0 MiB/sec, display the value in kiB/sec.
        if self.0 < 1024 * 1024 {
            return write!(f, "{:.1}kiB/s", self.0 as f64 / 1024.0);
        }

        write!(f, "{:.1}MiB/s", self.0 as f64 / (1024.0 * 1024.0))
    }
}

#[test]
fn test_display_block_hash() {
    let genesis_hash: BlockHash =
        "000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f"
            .parse()
            .unwrap();
    assert_eq!(
        format!("{}", DisplayBlockHash(genesis_hash)),
        "0000…8ce26f".to_string()
    );
}
