use jsonrpsee::RpcModule;
use sc_service::SpawnTaskHandle;
use sc_utils::mpsc::TracingUnboundedSender;
use std::sync::Arc;
use subcoin_indexer::IndexerQuery;
use subcoin_network::NetworkApi;
use subcoin_primitives::TransactionIndex;
use subcoin_runtime::interface::OpaqueBlock;
use subcoin_service::FullClient;
use substrate_frame_rpc_system::{System as FrameSystem, SystemApiServer as _};

/// Configuration for the RPC module.
pub struct RpcConfig {
    /// Bitcoin network (mainnet, testnet, etc.).
    pub bitcoin_network: bitcoin::Network,
    /// Whether to enable Bitcoin Core compatible RPC methods.
    pub enable_bitcoind_rpc: bool,
}

/// Instantiate all full RPC extensions.
#[allow(clippy::too_many_arguments)]
pub fn gen_rpc_module(
    system_info: sc_rpc::system::SystemInfo,
    client: Arc<FullClient>,
    spawn_handle: SpawnTaskHandle,
    system_rpc_tx: TracingUnboundedSender<sc_rpc::system::Request<OpaqueBlock>>,
    network_api: Arc<dyn NetworkApi>,
    transaction_indexer: Arc<dyn TransactionIndex + Send + Sync>,
    indexer_query: Option<Arc<IndexerQuery>>,
    rpc_config: RpcConfig,
) -> Result<RpcModule<()>, sc_service::Error> {
    use sc_rpc::chain::ChainApiServer;
    use sc_rpc::state::{ChildStateApiServer, StateApiServer};
    use sc_rpc::system::SystemApiServer;

    let mut module = RpcModule::new(());

    let dummy_pool = Arc::new(subcoin_service::transaction_pool::new_dummy_pool());

    let task_executor = Arc::new(spawn_handle);

    let system = sc_rpc::system::System::new(system_info, system_rpc_tx).into_rpc();
    let chain = sc_rpc::chain::new_full(client.clone(), task_executor.clone()).into_rpc();
    let (state, child_state) = {
        let (state, child_state) = sc_rpc::state::new_full(client.clone(), task_executor);
        (state.into_rpc(), child_state.into_rpc())
    };
    let _frame_system = FrameSystem::new(client.clone(), dummy_pool).into_rpc();

    let into_service_error =
        |e: jsonrpsee::core::error::RegisterMethodError| sc_service::Error::Application(e.into());

    // Substrate RPCs.
    module.merge(system).map_err(into_service_error)?;
    module.merge(chain).map_err(into_service_error)?;
    module.merge(state).map_err(into_service_error)?;
    module.merge(child_state).map_err(into_service_error)?;
    // module.merge(frame_system).map_err(into_service_error)?;

    // Subcoin RPCs.
    subcoin_rpc::SubcoinRpc::<_, _, subcoin_service::TransactionAdapter>::new(
        client.clone(),
        network_api.clone(),
        transaction_indexer.clone(),
        indexer_query,
        rpc_config.bitcoin_network,
    )
    .merge_into(&mut module)
    .map_err(into_service_error)?;

    // Bitcoin Core compatible RPCs (for electrs compatibility).
    if rpc_config.enable_bitcoind_rpc {
        subcoin_rpc_bitcoind::BitcoindRpc::<_, _, subcoin_service::TransactionAdapter>::new(
            client,
            network_api,
            transaction_indexer,
            rpc_config.bitcoin_network,
        )
        .merge_into(&mut module)
        .map_err(into_service_error)?;

        tracing::info!("Bitcoin Core compatible RPC methods enabled");
    }

    Ok(module)
}
