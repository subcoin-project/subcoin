//! Bitcoin Core compatible mining/fee estimation RPC methods.
//!
//! Implements: estimatesmartfee

use crate::error::Error;
use crate::types::EstimateSmartFee;
use jsonrpsee::proc_macros::rpc;

/// Bitcoin Core compatible mining RPC API.
#[rpc(client, server)]
pub trait MiningApi {
    /// Estimates the approximate fee per kilobyte needed for a transaction
    /// to begin confirmation within conf_target blocks.
    ///
    /// Returns estimate fee rate in BTC/kvB.
    #[method(name = "estimatesmartfee", blocking)]
    fn estimate_smart_fee(
        &self,
        conf_target: u32,
        estimate_mode: Option<String>,
    ) -> Result<EstimateSmartFee, Error>;
}

/// Bitcoin Core compatible mining RPC implementation.
pub struct MiningRpc {
    // In the future, this could hold a reference to mempool for fee estimation
}

impl MiningRpc {
    /// Creates a new instance of [`MiningRpc`].
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for MiningRpc {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl MiningApiServer for MiningRpc {
    fn estimate_smart_fee(
        &self,
        conf_target: u32,
        _estimate_mode: Option<String>,
    ) -> Result<EstimateSmartFee, Error> {
        // For now, return a simple fallback fee rate.
        // TODO: Implement proper fee estimation based on mempool analysis.
        //
        // electrs handles -32603 error gracefully, so we can return an error
        // or a simple estimate.

        // Return a conservative estimate of 1 sat/vB = 0.00001 BTC/kvB
        // This is a minimum fee rate that should work for most cases.
        let feerate = Some(0.00001);

        Ok(EstimateSmartFee {
            feerate,
            errors: None,
            blocks: conf_target,
        })
    }
}
