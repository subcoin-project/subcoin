//! MuHash checkpoints for UTXO set verification.
//!
//! These checkpoints are derived from Bitcoin Core's `gettxoutsetinfo muhash <height>`
//! command and can be used to verify fast sync downloads.
//!
//! The checkpoint data is stored in `checkpoints.json` and loaded at compile time.
//! See that file for the Bitcoin Core version used to generate these values.

use serde::Deserialize;
use std::collections::HashMap;
use std::sync::LazyLock;

/// Parsed checkpoints indexed by height.
static CHECKPOINTS: LazyLock<HashMap<u32, Checkpoint>> = LazyLock::new(|| {
    let file: CheckpointFile = serde_json::from_str(include_str!("checkpoints.json"))
        .expect("Failed to parse checkpoints.json");

    file.checkpoints
        .into_iter()
        .map(|cp| {
            (
                cp.height,
                Checkpoint {
                    height: cp.height,
                    block_hash: cp.block_hash,
                    txouts: cp.txouts,
                    muhash: cp.muhash,
                    total_amount: cp.total_amount,
                },
            )
        })
        .collect()
});

/// Raw checkpoint data as stored in JSON.
#[derive(Debug, Deserialize)]
struct CheckpointData {
    height: u32,
    block_hash: String,
    txouts: u64,
    muhash: String,
    total_amount: f64,
}

/// Parsed checkpoint file structure.
#[derive(Debug, Deserialize)]
struct CheckpointFile {
    #[allow(dead_code)]
    metadata: CheckpointMetadata,
    checkpoints: Vec<CheckpointData>,
}

/// Metadata about the checkpoint data source.
/// These fields are intentionally kept for documentation/verification purposes.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct CheckpointMetadata {
    bitcoin_core_version: String,
    generated_at: String,
    command: String,
    notes: String,
}

/// A single checkpoint entry with all relevant data.
#[derive(Debug, Clone)]
pub struct Checkpoint {
    pub height: u32,
    pub block_hash: String,
    pub txouts: u64,
    pub muhash: String,
    pub total_amount: f64,
}

/// Get the full checkpoint data at a specific height.
///
/// Returns `None` if no checkpoint exists for the given height.
pub fn get_checkpoint(height: u32) -> Option<&'static Checkpoint> {
    CHECKPOINTS.get(&height)
}

/// Get just the MuHash checkpoint at a specific height.
///
/// Returns `None` if no checkpoint exists for the given height.
pub fn get_muhash_checkpoint(height: u32) -> Option<&'static str> {
    CHECKPOINTS.get(&height).map(|cp| cp.muhash.as_str())
}

/// Get just the UTXO count checkpoint at a specific height.
///
/// Returns `None` if no checkpoint exists for the given height.
pub fn get_utxo_count_checkpoint(height: u32) -> Option<u64> {
    CHECKPOINTS.get(&height).map(|cp| cp.txouts)
}

/// Find the nearest checkpoint height at or below the given height.
///
/// Returns `None` if no checkpoint exists at or below the given height.
pub fn nearest_checkpoint_height(height: u32) -> Option<u32> {
    CHECKPOINTS.keys().filter(|&&h| h <= height).max().copied()
}

/// Get all checkpoint heights in ascending order.
pub fn checkpoint_heights() -> Vec<u32> {
    let mut heights: Vec<u32> = CHECKPOINTS.keys().copied().collect();
    heights.sort();
    heights
}

/// Get the highest (latest) checkpoint height.
///
/// Returns `None` if no checkpoints exist.
pub fn highest_checkpoint_height() -> Option<u32> {
    CHECKPOINTS.keys().max().copied()
}

/// Get the highest checkpoint with all its data.
///
/// Returns `None` if no checkpoints exist.
pub fn highest_checkpoint() -> Option<&'static Checkpoint> {
    CHECKPOINTS
        .keys()
        .max()
        .and_then(|&height| CHECKPOINTS.get(&height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_checkpoints_loaded() {
        // Verify checkpoints are loaded from JSON
        assert!(!CHECKPOINTS.is_empty());
        assert!(CHECKPOINTS.len() >= 10, "Expected at least 10 checkpoints");
    }

    #[test]
    fn test_get_checkpoint() {
        let cp = get_checkpoint(100_000).expect("Checkpoint at 100000 should exist");
        assert_eq!(cp.height, 100_000);
        assert_eq!(cp.txouts, 71888);
        assert_eq!(
            cp.muhash,
            "86aa993b32b9df2afa1a2c4855d34911146ad77328a82887e058d23f01831e3e"
        );

        assert!(get_checkpoint(99_999).is_none());
    }

    #[test]
    fn test_genesis_checkpoint() {
        let cp = get_checkpoint(0).expect("Genesis checkpoint should exist");
        assert_eq!(cp.height, 0);
        assert_eq!(cp.txouts, 0, "Genesis coinbase is unspendable");
        assert_eq!(
            cp.muhash,
            "dd5ad2a105c2d29495f577245c357409002329b9f4d6182c0af3dc2f462555c8"
        );
    }

    #[test]
    fn test_get_muhash_checkpoint() {
        assert!(get_muhash_checkpoint(100_000).is_some());
        assert!(get_muhash_checkpoint(99_999).is_none());
    }

    #[test]
    fn test_nearest_checkpoint() {
        assert_eq!(nearest_checkpoint_height(0), Some(0));
        assert_eq!(nearest_checkpoint_height(50_000), Some(0));
        assert_eq!(nearest_checkpoint_height(100_000), Some(100_000));
        assert_eq!(nearest_checkpoint_height(150_000), Some(100_000));
    }

    #[test]
    fn test_checkpoint_heights() {
        let heights = checkpoint_heights();
        assert!(heights.len() >= 2);
        assert_eq!(heights[0], 0);

        // Verify sorted order
        for i in 1..heights.len() {
            assert!(heights[i] > heights[i - 1]);
        }
    }

    #[test]
    fn test_highest_checkpoint() {
        let cp = highest_checkpoint().expect("Should have at least one checkpoint");
        assert_eq!(cp.height, 840_000);
        assert_eq!(cp.txouts, 176948713);
    }
}
