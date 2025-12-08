//! Bitcoin Core compatible network RPC methods.
//!
//! Implements: getnetworkinfo

use crate::error::Error;
use crate::types::GetNetworkInfo;
use bitcoin::Network;
use jsonrpsee::proc_macros::rpc;

/// Bitcoin Core compatible network RPC API.
#[rpc(client, server)]
pub trait NetworkApi {
    /// Returns an object containing various state info regarding P2P networking.
    #[method(name = "getnetworkinfo", blocking)]
    fn get_network_info(&self) -> Result<GetNetworkInfo, Error>;
}

/// Bitcoin Core compatible network RPC implementation.
pub struct NetworkRpc {
    #[allow(dead_code)]
    network: Network,
    /// Number of connected peers (can be updated externally).
    num_peers: std::sync::atomic::AtomicU32,
}

impl NetworkRpc {
    /// Creates a new instance of [`NetworkRpc`].
    pub fn new(network: Network) -> Self {
        Self {
            network,
            num_peers: std::sync::atomic::AtomicU32::new(0),
        }
    }

    /// Updates the number of connected peers.
    pub fn set_num_peers(&self, num: u32) {
        self.num_peers
            .store(num, std::sync::atomic::Ordering::Relaxed);
    }
}

#[async_trait::async_trait]
impl NetworkApiServer for NetworkRpc {
    fn get_network_info(&self) -> Result<GetNetworkInfo, Error> {
        let num_peers = self.num_peers.load(std::sync::atomic::Ordering::Relaxed);

        // Return a minimal but valid response for electrs compatibility.
        // electrs checks:
        // - version >= 21_00_00
        // - networkactive == true
        // - relayfee for get_relay_fee()
        Ok(GetNetworkInfo {
            // Version 27.0.0 in Bitcoin Core format
            version: 270000,
            subversion: "/Subcoin:0.1.0/".to_string(),
            protocolversion: 70016, // Current Bitcoin protocol version
            localservices: "0000000000000409".to_string(), // NODE_NETWORK | NODE_WITNESS | NODE_NETWORK_LIMITED
            localrelay: true,
            timeoffset: 0,
            networkactive: true,
            connections: num_peers,
            connections_in: 0,
            connections_out: num_peers,
            relayfee: 0.00001, // 1 sat/vB in BTC/kvB
            incrementalfee: 0.00001,
            localaddresses: vec![],
            warnings: vec![],
        })
    }
}
