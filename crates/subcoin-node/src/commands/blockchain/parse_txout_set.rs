use crate::cli::subcoin_params::Chain;
use crate::commands::blockchain::build_shared_params;
use sc_cli::SharedParams;
use std::fmt::Write;
use std::path::PathBuf;

#[derive(Debug, clap::Parser)]
pub struct ParseTxoutSet {
    #[clap(short, long)]
    path: PathBuf,

    #[clap(short, long)]
    compute_addresses: bool,

    /// Specify the chain.
    #[arg(long, value_name = "CHAIN", default_value = "bitcoin-mainnet")]
    chain: Chain,
}

#[derive(Debug)]
pub struct ParseTxoutSetCmd {
    path: PathBuf,
    compute_addresses: bool,
    chain: Chain,
    pub(super) shared_params: SharedParams,
}

impl ParseTxoutSetCmd {
    #[allow(clippy::result_large_err)]
    pub fn execute(self) -> sc_cli::Result<()> {
        let Self {
            path,
            compute_addresses,
            chain,
            ..
        } = self;

        let network = match chain {
            Chain::BitcoinSignet => bitcoin::Network::Signet,
            Chain::BitcoinTestnet => bitcoin::Network::Testnet,
            Chain::BitcoinMainnet => bitcoin::Network::Bitcoin,
        };

        let dump = txoutset::Dump::new(
            path,
            if compute_addresses {
                txoutset::ComputeAddresses::Yes(network)
            } else {
                txoutset::ComputeAddresses::No
            },
        )
        .map_err(|err| sc_cli::Error::Application(Box::new(err)))?;

        println!(
            "Dump opened.\n Block Hash: {}\n UTXO Set Size: {}",
            dump.block_hash, dump.utxo_set_size
        );

        let mut addr_str = String::new();

        for txout in dump {
            addr_str.clear();

            let txoutset::TxOut {
                address,
                amount,
                height,
                is_coinbase,
                out_point,
                script_pubkey,
            } = txout;

            match (compute_addresses, address) {
                (true, Some(address)) => {
                    let _ = write!(addr_str, ",{address}");
                }
                (true, None) => {
                    let _ = write!(addr_str, ",");
                }
                (false, _) => {}
            }

            let is_coinbase = u8::from(is_coinbase);
            let amount = u64::from(amount);
            println!(
                "{out_point},{is_coinbase},{height},{amount},{}{addr_str}",
                hex::encode(script_pubkey.as_bytes()),
            );
        }

        Ok(())
    }
}

impl From<ParseTxoutSet> for ParseTxoutSetCmd {
    fn from(parse_txout_set: ParseTxoutSet) -> Self {
        let ParseTxoutSet {
            path,
            compute_addresses,
            chain,
        } = parse_txout_set;
        Self {
            path,
            compute_addresses,
            chain,
            shared_params: build_shared_params(chain, None),
        }
    }
}
