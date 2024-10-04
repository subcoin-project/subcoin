use crate::in_mem::InMemStore;
use crate::BalanceChanges;

pub mod btc;

pub enum BackendType {
    InMem,
    Postgres,
}

pub enum IndexerStore {
    InMem(InMemStore),
}

impl IndexerStore {
    pub fn new(backend_type: BackendType) -> Self {
        match backend_type {
            BackendType::InMem => Self::InMem(InMemStore::new()),
            BackendType::Postgres => todo!(),
        }
    }

    pub fn write_block_changes(&mut self, block_changes: BalanceChanges) {
        match self {
            Self::InMem(store) => store.write_balance_changes(block_changes),
        }
    }
}
