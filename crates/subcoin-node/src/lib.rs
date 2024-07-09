mod cli;
mod commands;
mod rpc;
mod substrate_cli;
mod transaction_pool;

pub use self::cli::run;

pub struct CoinStorageKey;

impl subcoin_primitives::CoinStorageKey for CoinStorageKey {
    fn storage_key(&self, txid: bitcoin::Txid, vout: u32) -> Vec<u8> {
        pallet_bitcoin::coin_storage_key::<subcoin_runtime::Runtime>(txid, vout)
    }
}

#[cfg(test)]
mod tests {
    use sc_cli::Database;

    #[test]
    fn rocksdb_disabled_in_substrate() {
        assert_eq!(
            Database::variants(),
            &["paritydb", "paritydb-experimental", "auto"],
        );
    }
}
