use jsonrpsee::RpcModule;
use sc_service::SpawnTaskHandle;
use sc_utils::mpsc::TracingUnboundedSender;
use std::sync::Arc;
use subcoin_network::NetworkHandle;
use subcoin_runtime::interface::OpaqueBlock;
use subcoin_service::FullClient;
use substrate_frame_rpc_system::{System as FrameSystem, SystemApiServer as _};

/// Instantiate all full RPC extensions.
pub fn gen_rpc_module(
    system_info: sc_rpc::system::SystemInfo,
    client: Arc<FullClient>,
    spawn_handle: SpawnTaskHandle,
    system_rpc_tx: TracingUnboundedSender<sc_rpc::system::Request<OpaqueBlock>>,
    deny_unsafe: sc_rpc::DenyUnsafe,
    network_handle: NetworkHandle,
) -> Result<RpcModule<()>, sc_service::Error> {
    use sc_rpc::chain::ChainApiServer;
    use sc_rpc::state::{ChildStateApiServer, StateApiServer};
    use sc_rpc::system::SystemApiServer;
    use subcoin_rpc::blockchain::{Blockchain, BlockchainApiServer};
    use subcoin_rpc::subcoin::{Subcoin, SubcoinApiServer};

    let mut module = RpcModule::new(());

    let dummy_pool = Arc::new(crate::transaction_pool::new_dummy_pool());

    let task_executor = Arc::new(spawn_handle);

    let system = sc_rpc::system::System::new(system_info, system_rpc_tx, deny_unsafe).into_rpc();
    let chain = sc_rpc::chain::new_full(client.clone(), task_executor.clone()).into_rpc();
    let (state, child_state) = {
        let (state, child_state) =
            sc_rpc::state::new_full(client.clone(), task_executor, deny_unsafe);
        (state.into_rpc(), child_state.into_rpc())
    };
    let _frame_system = FrameSystem::new(client.clone(), dummy_pool, deny_unsafe).into_rpc();

    let into_service_error =
        |e: jsonrpsee::core::error::RegisterMethodError| sc_service::Error::Application(e.into());

    // Substrate RPCs.
    module.merge(system).map_err(into_service_error)?;
    module.merge(chain).map_err(into_service_error)?;
    module.merge(state).map_err(into_service_error)?;
    module.merge(child_state).map_err(into_service_error)?;
    // module.merge(frame_system).map_err(into_service_error)?;

    // Subcoin RPCs.
    let blockchain =
        Blockchain::<_, _, subcoin_service::TransactionAdapter>::new(client.clone()).into_rpc();
    let subcoin = Subcoin::new(client.clone(), network_handle).into_rpc();

    module.merge(blockchain).map_err(into_service_error)?;
    module.merge(subcoin).map_err(into_service_error)?;

    Ok(module)
}
