use clap::Args;
use sc_cli::{NetworkBackendType, NetworkParams, NodeKeyParams, NodeKeyType};
use sc_service::config::MultiaddrWithPeerId;
use std::num::NonZero;

/// Bitcoin chain type.
#[derive(Clone, Copy, Default, Debug, clap::ValueEnum)]
pub enum BitcoinChain {
    /// Bitcoin mainnet.
    #[default]
    Mainnet,
    /// Bitcoin testnet
    Testnet,
    /// Bitcoin signet.
    Signet,
}

impl BitcoinChain {
    /// Returns the value of `id` in `SubstrateCli::load_spec(id)`.
    pub fn chain_spec_id(&self) -> String {
        // Convert to kebab-case for consistency in CLI.
        match self {
            Self::Mainnet => "mainnet".to_string(),
            Self::Testnet => "testnet".to_string(),
            Self::Signet => "signet".to_string(),
        }
    }

    pub fn bitcoin_network(&self) -> bitcoin::Network {
        match self {
            Self::Mainnet => bitcoin::Network::Bitcoin,
            Self::Testnet => bitcoin::Network::Testnet,
            Self::Signet => bitcoin::Network::Signet,
        }
    }
}

/// Adapated [`NetworkParams`] to hide a bunch of unnecessary flags for a cleaner UTXO sync interface.
#[derive(Debug, Clone, Args)]
pub struct StateSyncNetworkParams {
    /// Specify a list of bootnodes.
    #[arg(long, value_name = "ADDR", num_args = 1..)]
    pub bootnodes: Vec<MultiaddrWithPeerId>,

    /// Specify p2p protocol TCP port.
    #[arg(long, value_name = "PORT")]
    pub port: Option<u16>,

    /// Network backend used for P2P networking.
    ///
    /// litep2p network backend is considered experimental and isn't as stable as the libp2p
    /// network backend.
    #[arg(
		long,
		value_enum,
		value_name = "NETWORK_BACKEND",
		default_value_t = NetworkBackendType::Libp2p,
		ignore_case = true,
		verbatim_doc_comment
	)]
    pub network_backend: NetworkBackendType,
}

impl StateSyncNetworkParams {
    pub fn into_network_params(self) -> NetworkParams {
        let Self {
            bootnodes,
            port,
            network_backend,
        } = self;

        NetworkParams {
            bootnodes,
            reserved_nodes: Vec::new(),
            reserved_only: false,
            public_addr: Vec::new(),
            listen_addr: Vec::new(),
            port,
            no_private_ip: false,
            allow_private_ip: true,
            out_peers: 8,
            in_peers: 32,
            in_peers_light: 100,
            no_mdns: true,
            max_parallel_downloads: 8,
            node_key_params: NodeKeyParams {
                node_key: None,
                node_key_type: NodeKeyType::Ed25519,
                node_key_file: None,
                unsafe_force_node_key_generation: false,
            },
            discover_local: false,
            kademlia_disjoint_query_paths: false,
            kademlia_replication_factor: NonZero::new(20).expect("20 is not zero"),
            ipfs_server: false,
            sync: sc_cli::SyncMode::Fast,
            max_blocks_per_request: 8,
            network_backend,
        }
    }
}
