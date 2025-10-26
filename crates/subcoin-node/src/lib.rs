//! Subcoin Node Library.
//!
//! The main feature of this library is to start and run the node as a CLI application.

#![allow(clippy::result_large_err)]

mod cli;
mod commands;
mod rpc;
#[cfg(feature = "remote-import")]
mod rpc_client;
mod substrate_cli;
mod utils;

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
