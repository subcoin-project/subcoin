//! UTXO cache layer for mempool validation.
//!
//! Implements a dual-layer cache:
//! - Base layer: UTXOs from native storage (invalidated on block import)
//! - Overlay: UTXOs created by pending mempool transactions (never cleared on block import)

use crate::error::MempoolError;
use bitcoin::{OutPoint as COutPoint, Transaction};
use schnellru::{ByLength, LruMap};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use subcoin_bitcoin_state::BitcoinState;
use subcoin_primitives::{MEMPOOL_HEIGHT, PoolCoin};

/// In-memory UTXO cache with native storage backend.
///
/// Uses a dual-layer design to handle both blockchain UTXOs and mempool-created coins:
/// - `base_cache`: LRU cache of UTXOs from the blockchain (cleared on block import)
/// - `mempool_overlay`: Coins created by pending transactions (preserved across blocks)
/// - `mempool_spends`: Outputs spent by pending transactions
pub struct CoinsViewCache {
    /// Base layer: coins from blockchain (invalidated on block import).
    base_cache: LruMap<COutPoint, Option<PoolCoin>, ByLength>,

    /// Overlay: coins created by mempool transactions (never cleared).
    mempool_overlay: HashMap<COutPoint, PoolCoin>,

    /// Outputs spent by mempool transactions.
    mempool_spends: HashSet<COutPoint>,

    /// Bitcoin state for O(1) UTXO lookups.
    bitcoin_state: Arc<BitcoinState>,
}

impl CoinsViewCache {
    /// Create a new coins view cache.
    ///
    /// # Arguments
    /// * `bitcoin_state` - Bitcoin state for O(1) UTXO lookups
    /// * `cache_size` - Maximum number of base layer entries to cache
    pub fn new(bitcoin_state: Arc<BitcoinState>, cache_size: u32) -> Self {
        Self {
            base_cache: LruMap::new(ByLength::new(cache_size)),
            mempool_overlay: HashMap::new(),
            mempool_spends: HashSet::new(),
            bitcoin_state,
        }
    }

    /// Get coin with overlay priority.
    ///
    /// Query order:
    /// 1. Check if spent by mempool
    /// 2. Check mempool overlay (created by pending txs)
    /// 3. Check base cache
    /// 4. Query native storage and cache result
    pub fn get_coin(&mut self, outpoint: &COutPoint) -> Result<Option<PoolCoin>, MempoolError> {
        // 1. Check mempool spends first
        if self.mempool_spends.contains(outpoint) {
            return Ok(None);
        }

        // 2. Check mempool overlay (created by pending transactions)
        if let Some(coin) = self.mempool_overlay.get(outpoint) {
            return Ok(Some(coin.clone()));
        }

        // 3. Check base cache
        if let Some(cached) = self.base_cache.peek(outpoint) {
            return Ok(cached.clone());
        }

        // 4. Fetch from native storage (O(1) lookup)
        let coin = self.fetch_from_storage(outpoint);
        self.base_cache.insert(*outpoint, coin.clone());
        Ok(coin)
    }

    /// Batch-prefetch coins (called at start of validation).
    ///
    /// Queries all needed coins and caches them for subsequent lookups.
    /// Should be called with all transaction inputs before validation begins.
    pub fn ensure_coins(&mut self, outpoints: &[COutPoint]) -> Result<(), MempoolError> {
        for outpoint in outpoints {
            if self.mempool_spends.contains(outpoint)
                || self.mempool_overlay.contains_key(outpoint)
                || self.base_cache.peek(outpoint).is_some()
            {
                continue;
            }

            // Fetch from native storage and cache
            let coin = self.fetch_from_storage(outpoint);
            self.base_cache.insert(*outpoint, coin);
        }

        Ok(())
    }

    /// Add coins from mempool transaction to overlay.
    ///
    /// These coins are marked with MEMPOOL_HEIGHT and are preserved across
    /// block imports until the transaction is removed from the mempool.
    pub fn add_mempool_coins(&mut self, tx: &Transaction) {
        for (idx, output) in tx.output.iter().enumerate() {
            let outpoint = COutPoint::new(tx.compute_txid(), idx as u32);
            self.mempool_overlay.insert(
                outpoint,
                PoolCoin {
                    output: output.clone(),
                    height: MEMPOOL_HEIGHT,
                    is_coinbase: false,
                    median_time_past: 0, // Mempool coins don't have MTP yet
                },
            );
        }
    }

    /// Mark coin as spent by mempool transaction.
    ///
    /// This creates an overlay that makes the coin appear spent even though
    /// it exists in the blockchain UTXO set.
    pub fn spend_coin(&mut self, outpoint: &COutPoint) {
        self.mempool_spends.insert(*outpoint);
    }

    /// Remove mempool transaction's coins (on eviction/confirmation).
    ///
    /// Cleans up both the coins created by this transaction and the spends
    /// it made from the overlay.
    pub fn remove_mempool_tx(&mut self, tx: &Transaction) {
        // Remove coins created by this transaction
        for idx in 0..tx.output.len() {
            let outpoint = COutPoint::new(tx.compute_txid(), idx as u32);
            self.mempool_overlay.remove(&outpoint);
        }

        // Remove spends made by this transaction
        for input in &tx.input {
            self.mempool_spends.remove(&input.previous_output);
        }
    }

    /// Update to new block tip.
    ///
    /// **CRITICAL:** Only clears the base cache, preserving the mempool overlay.
    /// Mempool coins remain valid until their transactions are removed.
    pub fn on_block_connected(&mut self) {
        self.base_cache.clear(); // Mempool overlay preserved
    }

    /// Check if coin exists (for quick lookups without fetching).
    pub fn have_coin(&self, outpoint: &COutPoint) -> bool {
        if self.mempool_spends.contains(outpoint) {
            return false;
        }

        self.mempool_overlay.contains_key(outpoint) || self.base_cache.peek(outpoint).is_some()
    }

    /// Fetch coin from Bitcoin state (O(1) lookup).
    fn fetch_from_storage(&self, outpoint: &COutPoint) -> Option<PoolCoin> {
        self.bitcoin_state.get(outpoint).map(|coin| PoolCoin {
            output: bitcoin::TxOut {
                value: bitcoin::Amount::from_sat(coin.amount),
                script_pubkey: bitcoin::ScriptBuf::from_bytes(coin.script_pubkey),
            },
            height: coin.height,
            is_coinbase: coin.is_coinbase,
            median_time_past: 0, // TODO: Store MTP in native storage if needed
        })
    }

    /// Get statistics about the cache.
    pub fn cache_stats(&self) -> CacheStats {
        CacheStats {
            base_cache_entries: self.base_cache.len(),
            mempool_overlay_entries: self.mempool_overlay.len(),
            mempool_spends_entries: self.mempool_spends.len(),
        }
    }
}

/// Statistics about the coins cache.
#[derive(Debug, Clone)]
pub struct CacheStats {
    /// Number of entries in base cache.
    pub base_cache_entries: usize,
    /// Number of entries in mempool overlay.
    pub mempool_overlay_entries: usize,
    /// Number of mempool spends tracked.
    pub mempool_spends_entries: usize,
}

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use super::*;

    // Note: Full tests require a mock native storage implementation.
}
