mod address;
mod blockchain;
mod error;
mod network;
mod raw_transactions;

use address::{Address, AddressApiServer};
use bitcoin::Network as BitcoinNetwork;
use blockchain::{Blockchain, BlockchainApiServer};
use network::{Network, NetworkApiServer};
use raw_transactions::{RawTransactions, RawTransactionsApiServer};
use sc_client_api::{AuxStore, BlockBackend, HeaderBackend};
use sp_runtime::traits::Block as BlockT;
use std::sync::Arc;
use subcoin_indexer::IndexerQuery;
use subcoin_network::NetworkApi;
use subcoin_primitives::{BitcoinTransactionAdapter, TransactionIndex};

/// Subcoin RPC.
pub struct SubcoinRpc<Block, Client, TransactionAdapter> {
    /// Blockchain RPC.
    pub blockchain: Blockchain<Block, Client, TransactionAdapter>,
    /// Raw transactions RPC.
    pub raw_transactions: RawTransactions<Block, Client>,
    /// Network RPC.
    pub network: Network<Block, Client>,
    /// Address RPC (only available when indexer is enabled).
    pub address: Option<Address<Block, Client>>,
}

impl<Block, Client, TransactionAdapter> SubcoinRpc<Block, Client, TransactionAdapter>
where
    Block: BlockT + 'static,
    Client: HeaderBackend<Block> + BlockBackend<Block> + AuxStore + Send + Sync + 'static,
    TransactionAdapter: BitcoinTransactionAdapter<Block> + Send + Sync + 'static,
{
    /// Creates a new instance of [`SubcoinRpc`].
    pub fn new(
        client: Arc<Client>,
        network_api: Arc<dyn NetworkApi>,
        transaction_indexer: Arc<dyn TransactionIndex + Send + Sync>,
        indexer_query: Option<Arc<IndexerQuery>>,
        bitcoin_network: BitcoinNetwork,
    ) -> Self {
        let address =
            indexer_query.map(|query| Address::<Block, Client>::new(client.clone(), query));

        Self {
            blockchain: Blockchain::<_, _, TransactionAdapter>::new(
                client.clone(),
                transaction_indexer,
                bitcoin_network,
            ),
            raw_transactions: RawTransactions::new(client.clone(), network_api.clone()),
            network: Network::new(client, network_api),
            address,
        }
    }

    /// Merges the Subcoin RPC components into a given RPC method registry.
    pub fn merge_into(
        self,
        module: &mut jsonrpsee::Methods,
    ) -> Result<(), jsonrpsee::server::RegisterMethodError> {
        let Self {
            blockchain,
            raw_transactions,
            network,
            address,
        } = self;

        module.merge(blockchain.into_rpc())?;
        module.merge(raw_transactions.into_rpc())?;
        module.merge(network.into_rpc())?;

        if let Some(address) = address {
            module.merge(address.into_rpc())?;
        }

        Ok(())
    }
}
