//! Bitcoin Core compatible mempool RPC methods.
//!
//! Implements: getrawmempool, getmempoolinfo, getmempoolentry

use crate::error::Error;
use crate::types::{GetMempoolEntry, GetMempoolInfo};
use bitcoin::Txid;
use jsonrpsee::proc_macros::rpc;
use std::collections::HashMap;

/// Bitcoin Core compatible mempool RPC API.
#[rpc(client, server)]
pub trait MempoolApi {
    /// Returns all transaction ids in memory pool.
    ///
    /// If verbose is false (default), returns array of transaction ids.
    /// If verbose is true, returns a JSON object with txid -> mempool entry.
    #[method(name = "getrawmempool", blocking)]
    fn get_raw_mempool(
        &self,
        verbose: Option<bool>,
        mempool_sequence: Option<bool>,
    ) -> Result<serde_json::Value, Error>;

    /// Returns details on the active state of the TX memory pool.
    #[method(name = "getmempoolinfo", blocking)]
    fn get_mempool_info(&self) -> Result<GetMempoolInfo, Error>;

    /// Returns mempool data for given transaction.
    #[method(name = "getmempoolentry", blocking)]
    fn get_mempool_entry(&self, txid: Txid) -> Result<GetMempoolEntry, Error>;
}

/// Bitcoin Core compatible mempool RPC implementation.
///
/// TODO: Integrate with subcoin-mempool crate for real mempool data.
pub struct MempoolRpc {
    // In the future, this will hold a reference to the actual mempool
}

impl MempoolRpc {
    /// Creates a new instance of [`MempoolRpc`].
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for MempoolRpc {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl MempoolApiServer for MempoolRpc {
    fn get_raw_mempool(
        &self,
        verbose: Option<bool>,
        _mempool_sequence: Option<bool>,
    ) -> Result<serde_json::Value, Error> {
        // TODO: Get actual mempool data from subcoin-mempool
        if verbose.unwrap_or(false) {
            // Return empty map
            let entries: HashMap<Txid, GetMempoolEntry> = HashMap::new();
            Ok(serde_json::to_value(entries)?)
        } else {
            // Return empty array of txids
            let txids: Vec<Txid> = vec![];
            Ok(serde_json::to_value(txids)?)
        }
    }

    fn get_mempool_info(&self) -> Result<GetMempoolInfo, Error> {
        // TODO: Get actual mempool info from subcoin-mempool
        Ok(GetMempoolInfo {
            loaded: true,
            size: 0,
            bytes: 0,
            usage: 0,
            total_fee: 0.0,
            maxmempool: 300_000_000, // 300 MB default
            mempoolminfee: 0.00001,  // 1 sat/vB in BTC/kvB
            minrelaytxfee: 0.00001,
            incrementalrelayfee: 0.00001,
            unbroadcastcount: 0,
            fullrbf: false,
        })
    }

    fn get_mempool_entry(&self, _txid: Txid) -> Result<GetMempoolEntry, Error> {
        // TODO: Get actual mempool entry from subcoin-mempool
        Err(Error::TransactionNotFound)
    }
}
