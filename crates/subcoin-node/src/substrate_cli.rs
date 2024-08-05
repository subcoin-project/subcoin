use subcoin_service::ChainSpec;

const BITCOIN_MAINNET_CHAIN_SPEC: &str = include_str!("../res/chain-spec-raw-bitcoin-mainnet.json");

/// Fake CLI for satisfying the Substrate CLI interface.
///
/// Primarily for creating a Substrate runner.
#[derive(Debug)]
pub struct SubstrateCli;

impl sc_cli::SubstrateCli for SubstrateCli {
    fn impl_name() -> String {
        "Subcoin Node".into()
    }

    fn impl_version() -> String {
        env!("SUBSTRATE_CLI_IMPL_VERSION").into()
    }

    fn description() -> String {
        env!("CARGO_PKG_DESCRIPTION").into()
    }

    fn author() -> String {
        env!("CARGO_PKG_AUTHORS").into()
    }

    fn support_url() -> String {
        "https://github.com/subcoin-project/subcoin/issues/new".into()
    }

    fn copyright_start_year() -> i32 {
        2024
    }

    fn load_spec(&self, id: &str) -> Result<Box<dyn sc_service::ChainSpec>, String> {
        let chain_spec = match id {
            "bitcoin-mainnet" => ChainSpec::from_json_bytes(BITCOIN_MAINNET_CHAIN_SPEC.as_bytes())?,
            "bitcoin-testnet" | "bitcoin-signet" => {
                unimplemented!("Bitcoin testnet and signet are unsupported")
            }
            path => ChainSpec::from_json_file(std::path::PathBuf::from(path))?,
        };

        Ok(Box::new(chain_spec))
    }
}
