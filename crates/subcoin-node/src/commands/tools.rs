use bitcoin::blockdata::script::Script;
use bitcoin::hashes::{sha256d, Hash};

/// Utilities
#[derive(Debug, clap::Subcommand)]
pub enum Tools {
    /// Revert a sha256 hash.
    ///
    /// Unlike the sha256 hash are displayed in the reversed format in the Bitcoin tools,
    /// H256 on polkadot.js.org are not. This is command is useful when you copy a hash
    /// from Bitcoin and want to paste it in polkadot.js.org.
    #[command(name = "revert-h256")]
    RevertH256 {
        #[arg(index = 1)]
        h256: String,
    },

    /// Parse the raw scriptPubkey of UTXO in the runtime.
    ///
    /// You can inspect the chain state from polkadot.js.org, copy the `scriptPubkey`
    /// in the storage and use this command to convert it to the human-readable
    /// Bitcoin Script format.
    #[command(name = "parse-script-pubkey-in-storage")]
    ParseScriptPubkeyInStorage {
        #[arg(index = 1)]
        input: String,
    },
}

fn revert_sha256d(h256d: &str) -> sc_cli::Result<String> {
    let hash: sha256d::Hash = h256d
        .parse()
        .map_err(|err| sc_cli::Error::Input(format!("Invalid h256: {err}")))?;

    Ok(hash
        .as_byte_array()
        .iter()
        .map(|byte| format!("{:02x}", byte))
        .collect::<Vec<_>>()
        .join(""))
}

impl Tools {
    pub fn run(self) -> sc_cli::Result<()> {
        match self {
            Self::RevertH256 { h256 } => {
                println!("Reverted: {}", revert_sha256d(&h256)?);
            }
            Self::ParseScriptPubkeyInStorage { input } => {
                let str_without_0x = input.strip_prefix("0x").unwrap_or(&input);

                let hex_bytes = hex::decode(str_without_0x)
                    .map_err(|err| sc_cli::Error::Input(format!("Invalid input: {err}")))?;

                let script = Script::from_bytes(&hex_bytes);
                println!("{script:?}");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_revert_sha256d() {
        assert_eq!(
            revert_sha256d("4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b")
                .unwrap(),
            "3ba3edfd7a7b12b27ac72c3e67768f617fc81bc3888a51323a9fb8aa4b1e5e4a".to_string()
        );
    }
}
