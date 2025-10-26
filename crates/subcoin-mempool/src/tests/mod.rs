//! Integration tests for mempool RBF and Package/CPFP functionality.

use bitcoin::hashes::Hash;
use bitcoin::{
    Address, Amount, Network, OutPoint, ScriptBuf, Transaction, TxIn, TxOut, Txid, absolute,
    transaction,
};
use sc_client_api::{BlockchainEvents, HeaderBackend};
use sp_api::ProvideRuntimeApi;
use sp_runtime::testing::{Block as TestBlock, Header, TestXt};
use sp_runtime::traits::{BlakeTwo256, Block as BlockT};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use subcoin_primitives::{BlockMetadata, PoolCoin};
// TODO: Implement proper runtime API mocks for SubcoinApi

use crate::{MemPool, MemPoolOptions};

// Type alias for test block with no-op extrinsics
type TestBlockType = TestBlock<TestXt<(), ()>>;

/// Minimal mock client for deterministic UTXO testing.
pub struct MockClient {
    /// UTXO set: OutPoint -> Coin
    utxos: RwLock<HashMap<OutPoint, PoolCoin>>,
    /// Current best block hash
    best_block: RwLock<<TestBlockType as BlockT>::Hash>,
    /// Current best block number
    best_number: RwLock<u64>,
}

impl MockClient {
    pub fn new() -> Self {
        Self {
            utxos: RwLock::new(HashMap::new()),
            best_block: RwLock::new(<TestBlockType as BlockT>::Hash::default()),
            best_number: RwLock::new(0),
        }
    }

    pub fn add_utxo(&self, outpoint: OutPoint, coin: PoolCoin) {
        self.utxos.write().unwrap().insert(outpoint, coin);
    }

    pub fn get_utxo(&self, outpoint: &OutPoint) -> Option<PoolCoin> {
        self.utxos.read().unwrap().get(outpoint).cloned()
    }
}

impl HeaderBackend<TestBlockType> for MockClient {
    fn header(
        &self,
        _hash: <TestBlockType as BlockT>::Hash,
    ) -> sp_blockchain::Result<Option<Header>> {
        Ok(Some(Header::new_from_number(
            *self.best_number.read().unwrap(),
        )))
    }

    fn info(&self) -> sc_client_api::blockchain::Info<TestBlockType> {
        sc_client_api::blockchain::Info {
            best_hash: *self.best_block.read().unwrap(),
            best_number: *self.best_number.read().unwrap(),
            finalized_hash: Default::default(),
            finalized_number: 0,
            genesis_hash: Default::default(),
            number_leaves: 1,
            finalized_state: None,
            block_gap: None,
        }
    }

    fn status(
        &self,
        _hash: <TestBlockType as BlockT>::Hash,
    ) -> sp_blockchain::Result<sc_client_api::blockchain::BlockStatus> {
        Ok(sc_client_api::blockchain::BlockStatus::InChain)
    }

    fn number(&self, _hash: <TestBlockType as BlockT>::Hash) -> sp_blockchain::Result<Option<u64>> {
        Ok(Some(*self.best_number.read().unwrap()))
    }

    fn hash(&self, _number: u64) -> sp_blockchain::Result<Option<<TestBlockType as BlockT>::Hash>> {
        Ok(Some(*self.best_block.read().unwrap()))
    }
}

impl ProvideRuntimeApi<TestBlockType> for MockClient {
    type Api = Self;

    fn runtime_api(&self) -> sp_api::ApiRef<Self::Api> {
        unimplemented!("Use direct SubcoinRuntimeApi trait methods")
    }
}

// TODO: Implement runtime API mocks using proper sp_api infrastructure
// The old SubcoinRuntimeApi has been removed and replaced with:
// - subcoin_runtime_primitives::SubcoinApi (runtime API for get_utxos)
// - subcoin_primitives::ClientExt (client-side for get_block_metadata, is_block_on_active_chain)

impl BlockchainEvents<TestBlockType> for MockClient {
    fn import_notification_stream(&self) -> sc_client_api::ImportNotifications<TestBlockType> {
        unimplemented!()
    }

    fn every_import_notification_stream(
        &self,
    ) -> sc_client_api::ImportNotifications<TestBlockType> {
        unimplemented!()
    }

    fn finality_notification_stream(&self) -> sc_client_api::FinalityNotifications<TestBlockType> {
        unimplemented!()
    }

    fn storage_changes_notification_stream(
        &self,
        _filter_keys: Option<&[sc_client_api::StorageKey]>,
        _child_filter_keys: Option<
            &[(
                sc_client_api::StorageKey,
                Option<Vec<sc_client_api::StorageKey>>,
            )],
        >,
    ) -> sp_blockchain::Result<sc_client_api::StorageEventStream<<TestBlockType as BlockT>::Hash>>
    {
        unimplemented!()
    }
}

/// Fluent transaction builder for tests.
pub struct TxBuilder {
    inputs: Vec<TxIn>,
    outputs: Vec<TxOut>,
}

impl TxBuilder {
    pub fn new() -> Self {
        Self {
            inputs: Vec::new(),
            outputs: Vec::new(),
        }
    }

    pub fn input(mut self, outpoint: OutPoint, sequence: u32) -> Self {
        self.inputs.push(TxIn {
            previous_output: outpoint,
            script_sig: ScriptBuf::default(),
            sequence: bitcoin::Sequence(sequence),
            witness: bitcoin::Witness::default(),
        });
        self
    }

    pub fn output(mut self, value: Amount, address: Address) -> Self {
        self.outputs.push(TxOut {
            value,
            script_pubkey: address.script_pubkey(),
        });
        self
    }

    pub fn build(self) -> Transaction {
        Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: self.inputs,
            output: self.outputs,
        }
    }
}

impl Default for TxBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper: Assert transaction is in mempool.
pub fn assert_in_mempool(mempool: &MemPool<TestBlockType, MockClient>, txid: &Txid) {
    assert!(
        mempool
            .inner
            .read()
            .unwrap()
            .arena
            .get_by_txid(txid)
            .is_some(),
        "Expected transaction {txid} to be in mempool"
    );
}

/// Helper: Assert transaction is NOT in mempool.
pub fn assert_not_in_mempool(mempool: &MemPool<TestBlockType, MockClient>, txid: &Txid) {
    assert!(
        mempool
            .inner
            .read()
            .unwrap()
            .arena
            .get_by_txid(txid)
            .is_none(),
        "Expected transaction {txid} to NOT be in mempool"
    );
}

/// Helper: Assert mempool size.
pub fn assert_mempool_size(mempool: &MemPool<TestBlockType, MockClient>, expected: usize) {
    let actual = mempool.size();
    assert_eq!(
        actual, expected,
        "Expected mempool size {expected}, got {actual}"
    );
}

/// Create a dummy address for tests.
pub fn dummy_address() -> Address {
    Address::from_script(
        &ScriptBuf::from_hex("76a914000000000000000000000000000000000000000088ac").unwrap(),
        Network::Bitcoin,
    )
    .unwrap()
}

/// Helper: Create a PoolCoin for testing.
pub fn create_coin(
    amount: Amount,
    script_pubkey: ScriptBuf,
    height: u32,
    is_coinbase: bool,
) -> PoolCoin {
    PoolCoin {
        output: TxOut {
            value: amount,
            script_pubkey,
        },
        height,
        is_coinbase,
        median_time_past: 0, // Default to 0 for tests (can be overridden when needed)
    }
}

// TODO: Fix MockClient ApiExt implementation for integration tests
// The infrastructure is set up but MockClient needs to implement ApiExt<Block>
// which is complex. For now, keep tests commented until after BIP68 is complete.
// #[cfg(test)]
// mod rbf_tests;

// #[cfg(test)]
// mod package_tests;
