mod cli;
mod commands;
mod rpc;
mod substrate_cli;
mod transaction_pool;

pub use self::cli::run;

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
