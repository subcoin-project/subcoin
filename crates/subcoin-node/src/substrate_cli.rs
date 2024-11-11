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
            "bitcoin-mainnet-compile" => {
                subcoin_service::chain_spec::config(bitcoin::Network::Bitcoin)?
            }
            "bitcoin-testnet" | "bitcoin-signet" => {
                unimplemented!("Bitcoin testnet and signet are unsupported")
            }
            path => ChainSpec::from_json_file(std::path::PathBuf::from(path))?,
        };

        Ok(Box::new(chain_spec))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_cli::SubstrateCli;
    use sc_client_api::HeaderBackend;
    use subcoin_service::{new_node, NodeComponents, SubcoinConfiguration};
    use tokio::runtime::Handle;

    #[tokio::test]
    async fn subcoin_mainnet_chain_is_stable_unless_breaking_changes() {
        let runtime_handle = Handle::current();
        let mainnet_chain_spec = SubstrateCli.load_spec("bitcoin-mainnet").unwrap();
        let config =
            subcoin_test_service::full_test_configuration(runtime_handle, mainnet_chain_spec);
        let NodeComponents { client, .. } =
            new_node(SubcoinConfiguration::test_config(&config)).expect("Failed to create node");

        assert_eq!(
            format!("{:?}", client.info().genesis_hash),
            "0x594ab67f8a784863e7597996bdc3bf37fee1ddc704a17897821a2b0507f99a58"
        );
    }
}
