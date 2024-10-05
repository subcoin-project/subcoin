mod btc;

use crate::in_mem::InMemStore;
use crate::postgres::PostgresStore;
use crate::BalanceChanges;

pub use btc::BtcIndexer;

pub enum BackendType {
    InMem,
    Postgres,
}

pub enum IndexerStore {
    InMem(InMemStore),
    Postgres(PostgresStore),
}

impl IndexerStore {
    pub fn new(backend_type: BackendType) -> Self {
        match backend_type {
            BackendType::InMem => Self::InMem(InMemStore::new()),
            BackendType::Postgres => Self::Postgres(PostgresStore::new()),
        }
    }

    pub fn write_block_changes(&mut self, block_changes: BalanceChanges) {
        match self {
            Self::InMem(store) => store.write_balance_changes(block_changes),
            Self::Postgres(store) => store
                .write_balance_changes(block_changes)
                .expect("Failed to write changes to postgres database"),
        }
    }
}
