//! Bitcoin Core compatible JSON-RPC API for Subcoin.
//!
//! This crate provides a Bitcoin Core compatible RPC layer that enables Subcoin
//! to work with Bitcoin ecosystem tools like electrs (Electrum server).
//!
//! # Supported RPC Methods
//!
//! ## Blockchain
//! - `getbestblockhash` - Returns the hash of the best block
//! - `getblockchaininfo` - Returns blockchain state info
//! - `getblockcount` - Returns the current block count
//! - `getblockhash` - Returns block hash at given height
//! - `getblock` - Returns block data (raw hex or JSON)
//! - `getblockheader` - Returns block header (raw hex or JSON)
//!
//! ## Raw Transactions
//! - `getrawtransaction` - Returns raw transaction data
//! - `sendrawtransaction` - Broadcasts a raw transaction
//! - `decoderawtransaction` - Decodes a raw transaction hex
//!
//! ## Network
//! - `getnetworkinfo` - Returns network state info
//!
//! ## Mempool
//! - `getrawmempool` - Returns all mempool txids
//! - `getmempoolinfo` - Returns mempool statistics
//! - `getmempoolentry` - Returns mempool entry for a txid
//!
//! ## Mining/Fees
//! - `estimatesmartfee` - Estimates fee rate for confirmation target

mod blockchain;
mod error;
mod mempool;
mod mining;
mod network;
mod rawtx;
mod types;

pub use blockchain::{Blockchain, BlockchainApiServer};
pub use error::Error;
pub use mempool::{MempoolApiServer, MempoolRpc};
pub use mining::{MiningApiServer, MiningRpc};
pub use network::{NetworkApiServer, NetworkRpc};
pub use rawtx::{RawTransaction, RawTransactionApiServer};
pub use types::*;

use bitcoin::Network;
use sc_client_api::{AuxStore, BlockBackend, HeaderBackend};
use sp_runtime::traits::Block as BlockT;
use std::sync::Arc;
use subcoin_network::NetworkApi;
use subcoin_primitives::{BitcoinTransactionAdapter, TransactionIndex};

/// Bitcoin Core compatible RPC server.
///
/// Aggregates all Bitcoin Core compatible RPC modules.
pub struct BitcoindRpc<Block, Client, TransactionAdapter> {
    /// Blockchain RPC.
    pub blockchain: Blockchain<Block, Client, TransactionAdapter>,
    /// Raw transaction RPC.
    pub rawtx: RawTransaction<Block, Client, TransactionAdapter>,
    /// Network RPC.
    pub network: NetworkRpc,
    /// Mempool RPC.
    pub mempool: MempoolRpc,
    /// Mining RPC.
    pub mining: MiningRpc,
}

impl<Block, Client, TransactionAdapter> BitcoindRpc<Block, Client, TransactionAdapter>
where
    Block: BlockT + 'static,
    Client: HeaderBackend<Block> + BlockBackend<Block> + AuxStore + 'static,
    TransactionAdapter: BitcoinTransactionAdapter<Block> + Send + Sync + 'static,
{
    /// Creates a new instance of [`BitcoindRpc`].
    pub fn new(
        client: Arc<Client>,
        network_api: Arc<dyn NetworkApi>,
        transaction_index: Arc<dyn TransactionIndex + Send + Sync>,
        network: Network,
    ) -> Self {
        Self {
            blockchain: Blockchain::<_, _, TransactionAdapter>::new(client.clone(), network),
            rawtx: RawTransaction::<_, _, TransactionAdapter>::new(
                client,
                network_api,
                transaction_index,
                network,
            ),
            network: NetworkRpc::new(network),
            mempool: MempoolRpc::new(),
            mining: MiningRpc::new(),
        }
    }

    /// Merges all RPC modules into a given RPC method registry.
    pub fn merge_into(
        self,
        module: &mut jsonrpsee::Methods,
    ) -> Result<(), jsonrpsee::server::RegisterMethodError> {
        let Self {
            blockchain,
            rawtx,
            network,
            mempool,
            mining,
        } = self;

        module.merge(blockchain.into_rpc())?;
        module.merge(rawtx.into_rpc())?;
        module.merge(network.into_rpc())?;
        module.merge(mempool.into_rpc())?;
        module.merge(mining.into_rpc())?;

        Ok(())
    }
}
