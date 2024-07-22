use bitcoin::consensus::Params;
use bitcoin::Network;

pub const MEDIAN_TIME_SPAN: usize = 11;

/// Extended [`Params`].
#[derive(Debug, Clone)]
pub struct ChainParams {
    /// Chain params defined in rust-bitcoin.
    pub params: Params,
    /// Block height at which CSV becomes active.
    pub csv_height: u32,
    /// Block height at which Segwit becomes active.
    pub segwit_height: u32,
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
            },
            Network::Testnet => Self {
                params,
                csv_height: 770112, // 00000000025e930139bac5c6c31a403776da130831ab85be56578f3fa75369bb
                segwit_height: 834624, // 00000000002b980fcd729daaa248fd9316a5200e9b367f4ff2c42453e84201ca
            },
            Network::Signet => Self {
                params,
                csv_height: 1,
                segwit_height: 1,
            },
            Network::Regtest => Self {
                params,
                csv_height: 1,    // Always active unless overridden
                segwit_height: 0, // Always active unless overridden
            },
            _ => unreachable!("Unknown Bitcoin Network"),
        }
    }
}
