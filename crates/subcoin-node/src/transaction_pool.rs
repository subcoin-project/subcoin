use futures::Future;
use sc_transaction_pool_api::{
    ImportNotificationStream, PoolFuture, PoolStatus, ReadyTransactions, TransactionFor,
    TransactionSource, TransactionStatusStreamFor, TxHash,
};
use sp_runtime::OpaqueExtrinsic;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use subcoin_runtime::interface::OpaqueBlock;

#[derive(Clone, Debug)]
pub struct PoolTransaction {
    data: Arc<OpaqueExtrinsic>,
    hash: sp_core::H256,
}

impl From<OpaqueExtrinsic> for PoolTransaction {
    fn from(e: OpaqueExtrinsic) -> Self {
        Self {
            data: Arc::from(e),
            hash: sp_core::H256::zero(),
        }
    }
}

impl sc_transaction_pool_api::InPoolTransaction for PoolTransaction {
    type Transaction = Arc<OpaqueExtrinsic>;
    type Hash = sp_core::H256;

    fn data(&self) -> &Self::Transaction {
        &self.data
    }

    fn hash(&self) -> &Self::Hash {
        &self.hash
    }

    fn priority(&self) -> &u64 {
        unimplemented!()
    }

    fn longevity(&self) -> &u64 {
        unimplemented!()
    }

    fn requires(&self) -> &[Vec<u8>] {
        unimplemented!()
    }

    fn provides(&self) -> &[Vec<u8>] {
        unimplemented!()
    }

    fn is_propagable(&self) -> bool {
        unimplemented!()
    }
}

#[derive(Clone, Debug)]
pub struct Transactions(Vec<Arc<PoolTransaction>>);
pub struct TransactionsIterator(std::vec::IntoIter<Arc<PoolTransaction>>);

/// Creates a dummy transaction pool.
pub fn new_dummy_pool() -> Transactions {
    Transactions(Vec::new())
}

impl Iterator for TransactionsIterator {
    type Item = Arc<PoolTransaction>;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }
}

impl ReadyTransactions for TransactionsIterator {
    fn report_invalid(&mut self, _tx: &Self::Item) {}
}

impl sc_transaction_pool_api::TransactionPool for Transactions {
    type Block = OpaqueBlock;
    type Hash = sp_core::H256;
    type InPoolTransaction = PoolTransaction;
    type Error = sc_transaction_pool_api::error::Error;

    /// Returns a future that imports a bunch of unverified transactions to the pool.
    fn submit_at(
        &self,
        _at: Self::Hash,
        _source: TransactionSource,
        _xts: Vec<TransactionFor<Self>>,
    ) -> PoolFuture<Vec<Result<sp_core::H256, Self::Error>>, Self::Error> {
        unimplemented!()
    }

    /// Returns a future that imports one unverified transaction to the pool.
    fn submit_one(
        &self,
        _at: Self::Hash,
        _source: TransactionSource,
        _xt: TransactionFor<Self>,
    ) -> PoolFuture<TxHash<Self>, Self::Error> {
        unimplemented!()
    }

    fn submit_and_watch(
        &self,
        _at: Self::Hash,
        _source: TransactionSource,
        _xt: TransactionFor<Self>,
    ) -> PoolFuture<Pin<Box<TransactionStatusStreamFor<Self>>>, Self::Error> {
        unimplemented!()
    }

    fn ready_at(
        &self,
        _at: Self::Hash,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Box<dyn ReadyTransactions<Item = Arc<Self::InPoolTransaction>> + Send>,
                > + Send,
        >,
    > {
        let iter: Box<dyn ReadyTransactions<Item = Arc<PoolTransaction>> + Send> =
            Box::new(TransactionsIterator(self.0.clone().into_iter()));
        Box::pin(futures::future::ready(iter))
    }

    fn ready(&self) -> Box<dyn ReadyTransactions<Item = Arc<Self::InPoolTransaction>> + Send> {
        unimplemented!()
    }

    fn remove_invalid(&self, _hashes: &[TxHash<Self>]) -> Vec<Arc<Self::InPoolTransaction>> {
        Default::default()
    }

    fn futures(&self) -> Vec<Self::InPoolTransaction> {
        unimplemented!()
    }

    fn status(&self) -> PoolStatus {
        unimplemented!()
    }

    fn import_notification_stream(&self) -> ImportNotificationStream<TxHash<Self>> {
        unimplemented!()
    }

    fn on_broadcasted(&self, _propagations: HashMap<TxHash<Self>, Vec<String>>) {
        unimplemented!()
    }

    fn hash_of(&self, _xt: &TransactionFor<Self>) -> TxHash<Self> {
        unimplemented!()
    }

    fn ready_transaction(&self, _hash: &TxHash<Self>) -> Option<Arc<Self::InPoolTransaction>> {
        unimplemented!()
    }

    fn ready_at_with_timeout(
        &self,
        _at: Self::Hash,
        _timeout: std::time::Duration,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Box<dyn ReadyTransactions<Item = Arc<Self::InPoolTransaction>> + Send>,
                > + Send
                + '_,
        >,
    > {
        unimplemented!()
    }
}
