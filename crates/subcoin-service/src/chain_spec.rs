use crate::ChainSpec;
use sc_service::{ChainType, Properties};
use serde_json::json;
use subcoin_primitives::raw_genesis_tx;
use subcoin_runtime::WASM_BINARY;

fn props() -> Properties {
    let mut properties = Properties::new();
    properties.insert("tokenDecimals".to_string(), 8.into());
    properties.insert("tokenSymbol".to_string(), "BTC".into());
    properties
}

pub fn config(network: bitcoin::Network) -> Result<ChainSpec, String> {
    let (name, id) = match network {
        bitcoin::Network::Bitcoin => ("Bitcoin", "mainnet"),
        bitcoin::Network::Testnet => ("Bitcoin Testnet", "testnet"),
        bitcoin::Network::Signet => ("Bitcoin Signet", "signet"),
        bitcoin::Network::Regtest => ("Bitcoin Regtest", "regtest"),
        unknown_network => unreachable!("Unknown Bitcoin network: {unknown_network:?}"),
    };
    Ok(ChainSpec::builder(
        WASM_BINARY.expect("Wasm binary not available"),
        Default::default(),
    )
    .with_name(name)
    .with_id(id)
    .with_chain_type(ChainType::Live)
    .with_genesis_config_patch(json!({
        "bitcoin": {
            "genesisTx": raw_genesis_tx(network),
        }
    }))
    .with_properties(props())
    .build())
}
