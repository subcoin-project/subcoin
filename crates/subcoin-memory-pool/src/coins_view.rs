//! UTXO cache layer for mempool validation.
//!
//! Implements a dual-layer cache:
//! - Base layer: UTXOs from the blockchain (invalidated on block import)
//! - Overlay: UTXOs created by pending mempool transactions (never cleared on block import)

use crate::types::MempoolError;
use bitcoin::{OutPoint as COutPoint, Transaction};
use sc_client_api::HeaderBackend;
use schnellru::{ByLength, LruMap};
use sp_api::ProvideRuntimeApi;
use sp_runtime::traits::Block as BlockT;
use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use std::sync::Arc;
use subcoin_primitives::{Coin, MEMPOOL_HEIGHT, SubcoinRuntimeApi};

/// In-memory UTXO cache with Substrate runtime backend.
///
/// Uses a dual-layer design to handle both blockchain UTXOs and mempool-created coins:
/// - `base_cache`: LRU cache of UTXOs from the blockchain (cleared on block import)
/// - `mempool_overlay`: Coins created by pending transactions (preserved across blocks)
/// - `mempool_spends`: Outputs spent by pending transactions
pub struct CoinsViewCache<Block: BlockT, Client> {
    /// Base layer: coins from blockchain (invalidated on block import).
    base_cache: LruMap<COutPoint, Option<Coin>, ByLength>,

    /// Overlay: coins created by mempool transactions (never cleared).
    mempool_overlay: HashMap<COutPoint, Coin>,

    /// Outputs spent by mempool transactions.
    mempool_spends: HashSet<COutPoint>,

    /// Best block hash (for runtime queries).
    best_block: Block::Hash,

    /// Substrate client for runtime API calls.
    client: Arc<Client>,

    _phantom: PhantomData<Block>,
}

impl<Block, Client> CoinsViewCache<Block, Client>
where
    Block: BlockT,
    Client: ProvideRuntimeApi<Block> + HeaderBackend<Block> + Send + Sync,
    Client::Api: SubcoinRuntimeApi<Block>,
{
    /// Create a new coins view cache.
    ///
    /// # Arguments
    /// * `client` - Substrate client for runtime API access
    /// * `cache_size` - Maximum number of base layer entries to cache
    pub fn new(client: Arc<Client>, cache_size: u32) -> Self {
        let best_block = client.info().best_hash;

        Self {
            base_cache: LruMap::new(ByLength::new(cache_size)),
            mempool_overlay: HashMap::new(),
            mempool_spends: HashSet::new(),
            best_block,
            client,
            _phantom: PhantomData,
        }
    }

    /// Get coin with overlay priority.
    ///
    /// Query order:
    /// 1. Check if spent by mempool
    /// 2. Check mempool overlay (created by pending txs)
    /// 3. Check base cache
    /// 4. Query runtime and cache result
    pub fn get_coin(&mut self, outpoint: &COutPoint) -> Result<Option<Coin>, MempoolError> {
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

        // 4. Fetch from runtime (single query)
        let coin = self.fetch_from_runtime(outpoint)?;
        self.base_cache.insert(*outpoint, coin.clone());
        Ok(coin)
    }

    /// Batch-prefetch coins (called at start of validation).
    ///
    /// This is critical for performance: queries all needed coins in a single
    /// runtime call rather than making individual calls per input.
    ///
    /// Should be called with all transaction inputs before validation begins.
    pub fn ensure_coins(&mut self, outpoints: &[COutPoint]) -> Result<(), MempoolError> {
        let missing: Vec<COutPoint> = outpoints
            .iter()
            .copied()
            .filter(|op| {
                !self.mempool_spends.contains(op)
                    && !self.mempool_overlay.contains_key(op)
                    && self.base_cache.peek(op).is_none()
            })
            .collect();

        if missing.is_empty() {
            return Ok(());
        }

        // Single batched runtime call for all missing coins
        let coins = self.batch_fetch_from_runtime(&missing)?;

        for (outpoint, coin_opt) in missing.into_iter().zip(coins) {
            self.base_cache.insert(outpoint, coin_opt);
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
                Coin {
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
    pub fn on_block_connected(&mut self, block_hash: Block::Hash) {
        self.best_block = block_hash;
        self.base_cache.clear(); // Mempool overlay preserved
    }

    /// Check if coin exists (for quick lookups without fetching).
    pub fn have_coin(&self, outpoint: &COutPoint) -> bool {
        if self.mempool_spends.contains(outpoint) {
            return false;
        }

        self.mempool_overlay.contains_key(outpoint) || self.base_cache.peek(outpoint).is_some()
    }

    /// Synchronous runtime API call for single coin.
    fn fetch_from_runtime(&self, outpoint: &COutPoint) -> Result<Option<Coin>, MempoolError> {
        let api = self.client.runtime_api();

        api.batch_get_utxos(self.best_block, vec![*outpoint])
            .map_err(|e| MempoolError::RuntimeApi(format!("Failed to fetch UTXO: {e:?}")))?
            .into_iter()
            .next()
            .ok_or_else(|| MempoolError::RuntimeApi("Empty response from runtime API".into()))
    }

    /// Batch fetch multiple coins from runtime.
    fn batch_fetch_from_runtime(
        &self,
        outpoints: &[COutPoint],
    ) -> Result<Vec<Option<Coin>>, MempoolError> {
        let api = self.client.runtime_api();

        api.batch_get_utxos(self.best_block, outpoints.to_vec())
            .map_err(|e| MempoolError::RuntimeApi(format!("Batch fetch failed: {e:?}")))
    }

    /// Get the current best block hash.
    pub fn best_block(&self) -> Block::Hash {
        self.best_block
    }

    /// Get reference to the client.
    pub fn client(&self) -> &Arc<Client> {
        &self.client
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
    use super::*;

    // Note: Full tests require a mock runtime API implementation.
    // These will be added when the runtime implementation is complete.
}
