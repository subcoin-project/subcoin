use subcoin_service::ChainSpec;

const BITCOIN_MAINNET_CHAIN_SPEC: &str = include_str!("../res/chain-spec-raw-bitcoin-mainnet.json");

fn chain_spec_bitcoin_mainnet() -> Result<ChainSpec, String> {
    ChainSpec::from_json_bytes(BITCOIN_MAINNET_CHAIN_SPEC.as_bytes())
}

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
        // TODO: different chain spec for different bitcoin network.
        // parse network from id
        // The chain spec here does not impact the chain but only for showing the proper
        // network type on explorer.

        let chain_spec = match id {
            "bitcoin-mainnet" => chain_spec_bitcoin_mainnet()?,
            path => ChainSpec::from_json_file(std::path::PathBuf::from(path))?,
        };

        Ok(Box::new(chain_spec))
    }
}
