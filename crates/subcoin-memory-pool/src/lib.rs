//! # Bitcoin Mempool Overview
//!
//! 1. Transaction Validation.
//!     - Transactions are validated before being added to the mempool.
//!     - Validation includes checking transaction size, fees and script validity.
//! 2. Fee Management
//!     - Transactions are prioritized based on their fee rate.
//!     - The mempool may evict lower-fee transactions if it reaches its size limit.
//! 3. Ancestors and Descendants.
//!     - The mempool tracks transaction dependencies to ensure that transactions are minded the
//!     correct order.

mod arena;
mod coins_view;
mod inner;
mod options;
mod policy;
mod types;
mod validation;

pub use self::arena::{MemPoolArena, TxMemPoolEntry};
pub use self::coins_view::CoinsViewCache;
pub use self::inner::MemPoolInner;
pub use self::options::MemPoolOptions;
pub use self::types::{
    EntryId, FeeRate, LockPoints, MempoolError, RemovalReason, ValidationResult,
};

use bitcoin::Transaction;
use sc_client_api::HeaderBackend;
use sp_api::ProvideRuntimeApi;
use sp_runtime::traits::Block as BlockT;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use subcoin_primitives::SubcoinRuntimeApi;

/// Thread-safe Bitcoin mempool.
///
/// Uses RwLock for interior mutability with the following lock hierarchy:
/// 1. MemPool::inner (RwLock)
/// 2. MemPool::coins_cache (RwLock)
/// 3. Runtime state backend (internal to client)
///
/// **CRITICAL:** Always acquire locks in this order to avoid deadlocks.
pub struct MemPool<Block: BlockT, Client> {
    /// Configuration (immutable after creation).
    options: MemPoolOptions,

    /// Thread-safe inner state.
    inner: RwLock<MemPoolInner>,

    /// UTXO cache (separate lock to reduce contention).
    coins_cache: RwLock<CoinsViewCache<Block, Client>>,

    /// Atomic counters (lockless).
    transactions_updated: AtomicU32,
    sequence_number: AtomicU64,

    /// Substrate client for runtime API access.
    client: Arc<Client>,

    _phantom: PhantomData<Block>,
}

impl<Block, Client> MemPool<Block, Client>
where
    Block: BlockT,
    Client: ProvideRuntimeApi<Block> + HeaderBackend<Block> + Send + Sync,
    Client::Api: SubcoinRuntimeApi<Block>,
{
    /// Create a new mempool with default options.
    pub fn new(client: Arc<Client>) -> Self {
        Self::with_options(client, MemPoolOptions::default())
    }

    /// Create a new mempool with custom options.
    pub fn with_options(client: Arc<Client>, options: MemPoolOptions) -> Self {
        let coins_cache = CoinsViewCache::new(client.clone(), 10_000);

        Self {
            options,
            inner: RwLock::new(MemPoolInner::new()),
            coins_cache: RwLock::new(coins_cache),
            transactions_updated: AtomicU32::new(0),
            sequence_number: AtomicU64::new(1),
            client,
            _phantom: PhantomData,
        }
    }

    /// Accept a single transaction into the mempool.
    ///
    /// **CRITICAL:** Holds write lock for entire ATMP flow to prevent TOCTOU races.
    pub fn accept_single_transaction(&self, tx: Transaction) -> Result<(), MempoolError> {
        // Acquire both locks for entire validation + commit (prevents TOCTOU)
        let mut inner = self.inner.write().expect("MemPool lock poisoned");
        let mut coins = self.coins_cache.write().expect("CoinsCache lock poisoned");

        // Get current chain state
        let best_block = coins.best_block();
        let current_height: u32 = self
            .client
            .info()
            .best_number
            .try_into()
            .unwrap_or_else(|_| panic!("Block number must fit into u32"));
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("Time went backwards")
            .as_secs() as i64;

        // Create validation workspace
        let mut ws = validation::ValidationWorkspace::new(tx);

        // Stage 1: PreChecks (sanity, standardness, input availability, fees)
        validation::pre_checks(
            &mut ws,
            &inner,
            &mut coins,
            &self.options,
            current_height,
            best_block,
        )?;

        // Stage 2: Check package limits (ancestors/descendants)
        validation::check_package_limits(&ws, &inner, &self.options)?;

        // TODO: Stage 3: PolicyScriptChecks (standard script validation)
        // TODO: Stage 4: ConsensusScriptChecks (consensus script validation)

        // Stage 5: Finalize - add to mempool
        let sequence = self.sequence_number.fetch_add(1, Ordering::SeqCst);
        let _entry_id = validation::finalize_tx(
            ws,
            &mut inner,
            &mut coins,
            current_height,
            current_time,
            sequence,
        )?;

        // Increment transactions_updated counter
        self.transactions_updated.fetch_add(1, Ordering::SeqCst);

        Ok(())
    }

    /// Get number of transactions in mempool.
    pub fn size(&self) -> usize {
        self.inner.read().expect("MemPool lock poisoned").size()
    }

    /// Get total size of all transactions in bytes.
    pub fn total_size(&self) -> u64 {
        self.inner
            .read()
            .expect("MemPool lock poisoned")
            .total_size()
    }

    /// Trim mempool to maximum size.
    pub fn trim_to_size(&self, max_size: u64) {
        self.inner
            .write()
            .expect("MemPool lock poisoned")
            .trim_to_size(max_size);
    }

    /// Expire old transactions.
    pub fn expire(&self, max_age_seconds: i64) {
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("Time went backwards")
            .as_secs() as i64;

        self.inner
            .write()
            .expect("MemPool lock poisoned")
            .expire(current_time, max_age_seconds);
    }

    /// Get mempool options.
    pub fn options(&self) -> &MemPoolOptions {
        &self.options
    }
}
