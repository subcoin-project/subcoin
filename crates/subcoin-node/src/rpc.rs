use jsonrpsee::RpcModule;
use sc_service::SpawnTaskHandle;
use sc_utils::mpsc::TracingUnboundedSender;
use std::sync::Arc;
use subcoin_network::NetworkApi;
use subcoin_runtime::interface::OpaqueBlock;
use subcoin_service::FullClient;
use substrate_frame_rpc_system::{System as FrameSystem, SystemApiServer as _};

/// Instantiate all full RPC extensions.
pub fn gen_rpc_module(
    system_info: sc_rpc::system::SystemInfo,
    client: Arc<FullClient>,
    spawn_handle: SpawnTaskHandle,
    system_rpc_tx: TracingUnboundedSender<sc_rpc::system::Request<OpaqueBlock>>,
    network_api: Arc<dyn NetworkApi>,
) -> Result<RpcModule<()>, sc_service::Error> {
    use sc_rpc::chain::ChainApiServer;
    use sc_rpc::state::{ChildStateApiServer, StateApiServer};
    use sc_rpc::system::SystemApiServer;

    let mut module = RpcModule::new(());

    let dummy_pool = Arc::new(crate::transaction_pool::new_dummy_pool());

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
        network_api,
    )
    .merge_into(&mut module)
    .map_err(into_service_error)?;

    Ok(module)
}
