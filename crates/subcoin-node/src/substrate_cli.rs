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
        "https://github.com/liuchengxu/subcoin/issues/new".into()
    }

    fn copyright_start_year() -> i32 {
        2024
    }

    fn load_spec(&self, _id: &str) -> Result<Box<dyn sc_service::ChainSpec>, String> {
        // TODO: different chain spec for different bitcoin network.
        // parse network from id
        // The chain spec here does not impact the chain but only for showing the proper
        // network type on explorer.
        Ok(Box::new(subcoin_service::chain_spec::config(
            bitcoin::Network::Bitcoin,
        )?))
    }
}
