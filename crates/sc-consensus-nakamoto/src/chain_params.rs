use bitcoin::consensus::Params;
use bitcoin::{BlockHash, Network};
use std::collections::HashMap;

/// Extended [`Params`].
#[derive(Debug, Clone)]
pub struct ChainParams {
    /// Chain params defined in rust-bitcoin.
    pub params: Params,
    /// Block height at which CSV becomes active.
    pub csv_height: u32,
    /// Block height at which Segwit becomes active.
    pub segwit_height: u32,
    /// A map of block hashes to script verification flag exceptions.
    ///
    /// This allows for certain blocks to have specific script verification flags, overriding
    /// the default rules. For example, exceptions may be made for blocks that activated
    /// BIP16 (P2SH) or Taproot under special conditions.
    pub script_flag_exceptions: HashMap<BlockHash, u32>,
}

impl ChainParams {
    /// Constructs a new instance of [`ChainParams`].
    // https://github.com/bitcoin/bitcoin/blob/6f9db1ebcab4064065ccd787161bf2b87e03cc1f/src/kernel/chainparams.cpp#L71
    pub fn new(network: Network) -> Self {
        let params = Params::new(network);
        match network {
            Network::Bitcoin => Self {
                params,
                csv_height: 419328, // 000000000000000004a1b34462cb8aeebd5799177f7a29cf28f2d1961716b5b5
                segwit_height: 481824, // 0000000000000000001c8018d9cb3b742ef25114f27563e3fc4a1902167f9893
                script_flag_exceptions: [
                    // BIP16 exception
                    (
                        "00000000000002dc756eebf4f49723ed8d30cc28a5f108eb94b1ba88ac4f9c22",
                        bitcoinconsensus::VERIFY_NONE,
                    ),
                    // Taproot exception
                    (
                        "0000000000000000000f14c35b2d841e986ab5441de8c585d5ffe55ea1e395ad",
                        bitcoinconsensus::VERIFY_P2SH | bitcoinconsensus::VERIFY_WITNESS,
                    ),
                ]
                .into_iter()
                .map(|(block_hash, flag)| {
                    (block_hash.parse().expect("Hash must be valid; qed"), flag)
                })
                .collect(),
            },
            Network::Testnet => Self {
                params,
                csv_height: 770112, // 00000000025e930139bac5c6c31a403776da130831ab85be56578f3fa75369bb
                segwit_height: 834624, // 00000000002b980fcd729daaa248fd9316a5200e9b367f4ff2c42453e84201ca
                script_flag_exceptions: HashMap::from_iter([
                    // BIP16 exception
                    (
                        "00000000dd30457c001f4095d208cc1296b0eed002427aa599874af7a432b105"
                            .parse()
                            .expect("Hash must be valid; qed"),
                        bitcoinconsensus::VERIFY_NONE,
                    ),
                ]),
            },
            Network::Signet => Self {
                params,
                csv_height: 1,
                segwit_height: 1,
                script_flag_exceptions: Default::default(),
            },
            Network::Regtest => Self {
                params,
                csv_height: 1,    // Always active unless overridden
                segwit_height: 0, // Always active unless overridden
                script_flag_exceptions: Default::default(),
            },
            _ => unreachable!("Unknown Bitcoin Network"),
        }
    }
}
