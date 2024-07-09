use parking_lot::RwLock;
use sp_core::storage::{well_known_keys, ChildInfo};
use sp_runtime::traits::{Block as BlockT, HashingFor};
use sp_runtime::StateVersion;
use sp_state_machine::backend::AsTrieBackend;
use sp_state_machine::{
    Backend as StateBackend, BackendTransaction, ChildStorageCollection, InMemoryBackend, IterArgs,
    StateMachineStats, StorageCollection, StorageIterator, StorageKey, StorageValue,
};
use sp_trie::{MerkleValue, PrefixedMemoryDB};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// A raw iterator over the `RefTrackingState`.
pub struct RawIter<Block: BlockT> {
    inner: <InMemoryBackend<HashingFor<Block>> as StateBackend<HashingFor<Block>>>::RawIter,
}

impl<Block: BlockT> StorageIterator<HashingFor<Block>> for RawIter<Block> {
    type Backend = ChainState<Block>;
    type Error = <ChainState<Block> as StateBackend<HashingFor<Block>>>::Error;

    fn next_key(&mut self, backend: &Self::Backend) -> Option<Result<StorageKey, Self::Error>> {
        self.inner.next_key(&backend.state.read())
    }

    fn next_pair(
        &mut self,
        backend: &Self::Backend,
    ) -> Option<Result<(StorageKey, StorageValue), Self::Error>> {
        self.inner.next_pair(&backend.state.read())
    }

    fn was_complete(&self) -> bool {
        self.inner.was_complete()
    }
}

/// Storage transactions are calculated as part of the `storage_root`.
/// These transactions can be reused for importing the block into the
/// storage. So, we cache them to not require a recomputation of those transactions.
struct StorageTransactionCache<Block: BlockT> {
    /// Contains the changes for the main and the child storages as one transaction.
    transaction: BackendTransaction<HashingFor<Block>>,
    /// The storage root after applying the transaction.
    transaction_storage_root: Block::Hash,
}

impl<Block: BlockT> StorageTransactionCache<Block> {
    fn into_inner(self) -> (Block::Hash, BackendTransaction<HashingFor<Block>>) {
        (self.transaction_storage_root, self.transaction)
    }
}

impl<Block: BlockT> Clone for StorageTransactionCache<Block> {
    fn clone(&self) -> Self {
        Self {
            transaction: self.transaction.clone(),
            transaction_storage_root: self.transaction_storage_root,
        }
    }
}

impl<Block: BlockT> core::fmt::Debug for StorageTransactionCache<Block> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut debug = f.debug_struct("StorageTransactionCache");

        debug.field("transaction_storage_root", &self.transaction_storage_root);

        debug.finish()
    }
}

/// Latest state of subcoin, representing the state of UTXO set.
#[derive(Debug, Clone)]
pub struct ChainState<Block: BlockT> {
    runtime_hash: Block::Hash,
    runtime_code: Arc<Vec<u8>>,
    pub(crate) state: Arc<RwLock<InMemoryBackend<HashingFor<Block>>>>,
    read_storage_root_count: Arc<AtomicUsize>,
    storage_transaction_cache: Arc<RwLock<Option<StorageTransactionCache<Block>>>>,
}

impl<Block: BlockT> ChainState<Block> {
    pub fn new(runtime_hash: Block::Hash, runtime_code: Vec<u8>) -> Self {
        Self {
            runtime_hash,
            runtime_code: Arc::new(runtime_code),
            state: Arc::new(RwLock::new(Default::default())),
            read_storage_root_count: Arc::new(AtomicUsize::new(0)),
            storage_transaction_cache: Arc::new(RwLock::new(None)),
        }
    }

    pub fn reset_storage_root(&self) {
        self.read_storage_root_count.store(0, Ordering::SeqCst);
        *self.storage_transaction_cache.write() = None;
    }

    pub fn apply_transaction(
        &mut self,
        root: Block::Hash,
        transaction: BackendTransaction<HashingFor<Block>>,
    ) {
        self.state.write().apply_transaction(root, transaction);
    }
}

impl<B: BlockT> AsTrieBackend<HashingFor<B>> for ChainState<B> {
    type TrieBackendStorage = PrefixedMemoryDB<HashingFor<B>>;

    fn as_trie_backend(
        &self,
    ) -> &sp_state_machine::TrieBackend<Self::TrieBackendStorage, HashingFor<B>> {
        unimplemented!(
            "Implementing this trait is not easy for ChainState, \
            but luckily it's not necessary for the block execution"
        )
    }
}

impl<Block: BlockT> StateBackend<HashingFor<Block>> for ChainState<Block> {
    type Error = <InMemoryBackend<HashingFor<Block>> as StateBackend<HashingFor<Block>>>::Error;
    type TrieBackendStorage =
        <InMemoryBackend<HashingFor<Block>> as StateBackend<HashingFor<Block>>>::TrieBackendStorage;
    type RawIter = RawIter<Block>;

    fn storage(&self, key: &[u8]) -> Result<Option<StorageValue>, Self::Error> {
        if key == well_known_keys::CODE {
            return Ok(Some(self.runtime_code.to_vec()));
        }

        let now = std::time::Instant::now();
        let res = self.state.read().storage(key);
        tracing::debug!(target: "in_mem", "read {:?} took {}ns", 
			sp_core::hexdisplay::HexDisplay::from(&key),
            now.elapsed().as_nanos());
        res
    }

    fn storage_hash(&self, key: &[u8]) -> Result<Option<Block::Hash>, Self::Error> {
        if key == well_known_keys::CODE {
            return Ok(Some(self.runtime_hash));
        }

        self.state.read().storage_hash(key)
    }

    fn closest_merkle_value(
        &self,
        key: &[u8],
    ) -> Result<Option<MerkleValue<Block::Hash>>, Self::Error> {
        self.state.read().closest_merkle_value(key)
    }

    fn child_closest_merkle_value(
        &self,
        _child_info: &ChildInfo,
        _key: &[u8],
    ) -> Result<Option<MerkleValue<Block::Hash>>, Self::Error> {
        unimplemented!()
    }

    fn child_storage(
        &self,
        _child_info: &ChildInfo,
        _key: &[u8],
    ) -> Result<Option<StorageValue>, Self::Error> {
        unimplemented!()
    }

    fn child_storage_hash(
        &self,
        _child_info: &ChildInfo,
        _key: &[u8],
    ) -> Result<Option<Block::Hash>, Self::Error> {
        unimplemented!()
    }

    fn next_storage_key(&self, key: &[u8]) -> Result<Option<StorageKey>, Self::Error> {
        self.state.read().next_storage_key(key)
    }

    fn next_child_storage_key(
        &self,
        _child_info: &ChildInfo,
        _key: &[u8],
    ) -> Result<Option<StorageKey>, Self::Error> {
        unimplemented!()
    }

    fn storage_root<'a>(
        &self,
        delta: impl Iterator<Item = (&'a [u8], Option<&'a [u8]>)>,
        state_version: StateVersion,
    ) -> (Block::Hash, BackendTransaction<HashingFor<Block>>)
    where
        Block::Hash: Ord,
    {
        if let Some(cached) = self.storage_transaction_cache.read().as_ref() {
            return cached.clone().into_inner();
        }

        let old = self.read_storage_root_count.fetch_add(1, Ordering::SeqCst);
        let now = std::time::Instant::now();
        let res = self.state.read().storage_root(delta, state_version);
        // FIXME: there are two storage_root operations, but runtime disk has only one, figure it out
        tracing::debug!(
            "[in_mem::storage_root] storage_root: {}, elapsed {} ns, count: {}",
            res.0,
            now.elapsed().as_nanos(),
            old + 1
        );
        let (transaction_storage_root, transaction) = res.clone();
        *self.storage_transaction_cache.write() = Some(StorageTransactionCache {
            transaction_storage_root,
            transaction,
        });
        res
    }

    fn child_storage_root<'a>(
        &self,
        _child_info: &ChildInfo,
        _delta: impl Iterator<Item = (&'a [u8], Option<&'a [u8]>)>,
        _state_version: StateVersion,
    ) -> (Block::Hash, bool, BackendTransaction<HashingFor<Block>>)
    where
        Block::Hash: Ord,
    {
        unimplemented!()
    }

    fn raw_iter(&self, args: IterArgs) -> Result<Self::RawIter, Self::Error> {
        self.state
            .read()
            .raw_iter(args)
            .map(|inner| RawIter { inner })
    }

    fn register_overlay_stats(&self, stats: &StateMachineStats) {
        self.state.read().register_overlay_stats(stats)
    }

    fn usage_info(&self) -> sp_state_machine::UsageInfo {
        sp_state_machine::UsageInfo::empty()
    }

    fn commit(
        &self,
        _: Block::Hash,
        _: BackendTransaction<HashingFor<Block>>,
        _: StorageCollection,
        _: ChildStorageCollection,
    ) -> Result<(), Self::Error> {
        unimplemented!()
    }

    fn read_write_count(&self) -> (u32, u32, u32, u32) {
        unimplemented!()
    }

    fn reset_read_write_count(&self) {
        unimplemented!()
    }
}
