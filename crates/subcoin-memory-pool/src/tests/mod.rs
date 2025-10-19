//! Integration tests for mempool RBF and Package/CPFP functionality.

use bitcoin::hashes::Hash;
use bitcoin::{absolute, transaction, Address, Amount, Network, OutPoint, ScriptBuf, Transaction, TxIn, TxOut, Txid};
use sc_client_api::{BlockchainEvents, HeaderBackend};
use sp_api::ProvideRuntimeApi;
use sp_runtime::traits::{BlakeTwo256, Block as BlockT};
use sp_runtime::testing::{Block as TestBlock, Header};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use subcoin_primitives::{BackendExt, Coin, SubcoinRuntimeApi};

use crate::{MemPool, MemPoolOptions};

/// Minimal mock client for deterministic UTXO testing.
pub struct MockClient {
    /// UTXO set: OutPoint -> Coin
    utxos: RwLock<HashMap<OutPoint, Coin>>,
    /// Current best block hash
    best_block: RwLock<<TestBlock as BlockT>::Hash>,
    /// Current best block number
    best_number: RwLock<u32>,
}

impl MockClient {
    pub fn new() -> Self {
        Self {
            utxos: RwLock::new(HashMap::new()),
            best_block: RwLock::new(<TestBlock as BlockT>::Hash::default()),
            best_number: RwLock::new(0),
        }
    }

    pub fn add_utxo(&self, outpoint: OutPoint, coin: Coin) {
        self.utxos.write().unwrap().insert(outpoint, coin);
    }

    pub fn get_utxo(&self, outpoint: &OutPoint) -> Option<Coin> {
        self.utxos.read().unwrap().get(outpoint).cloned()
    }
}

impl HeaderBackend<TestBlock> for MockClient {
    fn header(
        &self,
        _hash: <TestBlock as BlockT>::Hash,
    ) -> sp_blockchain::Result<Option<Header>> {
        Ok(Some(Header::new(
            *self.best_number.read().unwrap(),
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
        )))
    }

    fn info(&self) -> sc_client_api::blockchain::Info<TestBlock> {
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
        _hash: <TestBlock as BlockT>::Hash,
    ) -> sp_blockchain::Result<sc_client_api::blockchain::BlockStatus> {
        Ok(sc_client_api::blockchain::BlockStatus::InChain)
    }

    fn number(
        &self,
        _hash: <TestBlock as BlockT>::Hash,
    ) -> sp_blockchain::Result<Option<u32>> {
        Ok(Some(*self.best_number.read().unwrap()))
    }

    fn hash(&self, _number: u32) -> sp_blockchain::Result<Option<<TestBlock as BlockT>::Hash>> {
        Ok(Some(*self.best_block.read().unwrap()))
    }
}

impl ProvideRuntimeApi<TestBlock> for MockClient {
    type Api = Self;

    fn runtime_api(&self) -> sp_api::ApiRef<Self::Api> {
        unimplemented!("Use direct SubcoinRuntimeApi trait methods")
    }
}

impl SubcoinRuntimeApi<TestBlock> for MockClient {
    fn get_coin(&self, outpoint: OutPoint) -> Result<Option<Coin>, sp_api::ApiError> {
        Ok(self.get_utxo(&outpoint))
    }
}

impl BlockchainEvents<TestBlock> for MockClient {
    fn import_notification_stream(
        &self,
    ) -> sc_client_api::ImportNotifications<TestBlock> {
        unimplemented!()
    }

    fn every_import_notification_stream(
        &self,
    ) -> sc_client_api::ImportNotifications<TestBlock> {
        unimplemented!()
    }

    fn finality_notification_stream(
        &self,
    ) -> sc_client_api::FinalityNotifications<TestBlock> {
        unimplemented!()
    }

    fn storage_changes_notification_stream(
        &self,
        _filter_keys: Option<&[sc_client_api::StorageKey]>,
        _child_filter_keys: Option<
            &[(sc_client_api::StorageKey, Option<Vec<sc_client_api::StorageKey>>)],
        >,
    ) -> sp_blockchain::Result<sc_client_api::StorageEventStream<<TestBlock as BlockT>::Hash>> {
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
pub fn assert_in_mempool(mempool: &MemPool<TestBlock, MockClient>, txid: &Txid) {
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
pub fn assert_not_in_mempool(mempool: &MemPool<TestBlock, MockClient>, txid: &Txid) {
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
pub fn assert_mempool_size(mempool: &MemPool<TestBlock, MockClient>, expected: usize) {
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

/// Helper: Create a Coin for testing.
pub fn create_coin(amount: Amount, script_pubkey: ScriptBuf, height: u32, is_coinbase: bool) -> Coin {
    Coin {
        output: TxOut {
            value: amount,
            script_pubkey,
        },
        height,
        is_coinbase,
    }
}

// TODO: Fix integration tests - need to add missing MemPoolOptionsBuilder methods
// and fix MockClient implementation
// #[cfg(test)]
// mod rbf_tests;

// #[cfg(test)]
// mod package_tests;
