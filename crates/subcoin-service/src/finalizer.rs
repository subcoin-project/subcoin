use futures::StreamExt;
use parking_lot::Mutex;
use sc_client_api::{BlockchainEvents, Finalizer, HeaderBackend};
use sc_network_sync::SyncingService;
use sc_service::SpawnTaskHandle;
use sp_consensus::SyncOracle;
use sp_runtime::traits::{Block as BlockT, CheckedSub, NumberFor};
use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use subcoin_network::NetworkApi;

type BlockInfo<Block> = (NumberFor<Block>, <Block as BlockT>::Hash);

/// This struct is responsible for finalizing blocks with enough confirmations.
pub struct SubcoinFinalizer<Block: BlockT, Client, Backend> {
    client: Arc<Client>,
    spawn_handle: SpawnTaskHandle,
    confirmation_depth: u32,
    subcoin_network_api: Arc<dyn NetworkApi>,
    substrate_sync_service: Option<Arc<SyncingService<Block>>>,
    _phantom: PhantomData<Backend>,
}

impl<Block, Client, Backend> SubcoinFinalizer<Block, Client, Backend>
where
    Block: BlockT + 'static,
    Client: HeaderBackend<Block> + Finalizer<Block, Backend> + BlockchainEvents<Block> + 'static,
    Backend: sc_client_api::backend::Backend<Block> + 'static,
{
    /// Constructs a new instance of [`SubcoinFinalizer`].
    pub fn new(
        client: Arc<Client>,
        spawn_handle: SpawnTaskHandle,
        confirmation_depth: u32,
        subcoin_network_api: Arc<dyn NetworkApi>,
        substrate_sync_service: Option<Arc<SyncingService<Block>>>,
    ) -> Self {
        Self {
            client,
            spawn_handle,
            confirmation_depth,
            subcoin_network_api,
            substrate_sync_service,
            _phantom: Default::default(),
        }
    }

    /// The future needs to be spawned in the background.
    pub async fn run(self) {
        let Self {
            client,
            spawn_handle,
            confirmation_depth,
            subcoin_network_api,
            substrate_sync_service,
            _phantom,
        } = self;

        // Use `every_import_notification_stream()` so that we can receive the notifications even when
        // the Substrate network is major syncing.
        let mut block_import_stream = client.every_import_notification_stream();

        let cached_block_to_finalize: Arc<Mutex<Option<BlockInfo<Block>>>> =
            Arc::new(Mutex::new(None));

        let finalizer_worker_is_busy = Arc::new(AtomicBool::new(false));

        while let Some(notification) = block_import_stream.next().await {
            let block_number = client
                .number(notification.hash)
                .ok()
                .flatten()
                .expect("Imported block must be available; qed");

            let Some(confirmed_block_number) = block_number.checked_sub(&confirmation_depth.into())
            else {
                continue;
            };

            let finalized_number = client.info().finalized_number;

            if confirmed_block_number <= finalized_number {
                continue;
            }

            let confirmed_block_hash = client
                .hash(confirmed_block_number)
                .ok()
                .flatten()
                .expect("Confirmed block must be available; qed");

            let try_update_cached_block_to_finalize = || {
                let mut cached_block_to_finalize = cached_block_to_finalize.lock();

                let should_update = cached_block_to_finalize
                    .map(|(cached_block_number, _)| confirmed_block_number > cached_block_number)
                    .unwrap_or(true);

                if should_update {
                    cached_block_to_finalize
                        .replace((confirmed_block_number, confirmed_block_hash));
                }

                drop(cached_block_to_finalize);
            };

            if let Some(sync_service) = substrate_sync_service.as_ref() {
                // State sync relies on the finalized block notification to progress
                // Substrate chain sync component relies on the finalized block notification to
                // initiate the state sync, do not attempt to finalize the block when the queued blocks
                // are not empty, so that the state sync can be started when the last finalized block
                // notification is sent.
                if sync_service.is_major_syncing()
                    && sync_service
                        .status()
                        .await
                        .map(|status| status.queued_blocks)
                        .unwrap_or(0)
                        > 0
                {
                    try_update_cached_block_to_finalize();
                    continue;
                }
            }

            if finalizer_worker_is_busy.load(Ordering::SeqCst) {
                try_update_cached_block_to_finalize();
                continue;
            }

            let client = client.clone();
            let subcoin_network_api = subcoin_network_api.clone();
            let substrate_sync_service = substrate_sync_service.clone();
            let finalizer_worker_is_busy = finalizer_worker_is_busy.clone();
            let cached_block_to_finalize = cached_block_to_finalize.clone();

            finalizer_worker_is_busy.store(true, Ordering::SeqCst);

            spawn_handle.spawn(
                "finalize-block",
                None,
                Box::pin(async move {
                    do_finalize_block(
                        &client,
                        confirmed_block_number,
                        confirmed_block_hash,
                        &subcoin_network_api,
                        substrate_sync_service.as_ref(),
                    );

                    let mut cached_block_to_finalize = cached_block_to_finalize.lock();
                    let maybe_cached_block_to_finalize = cached_block_to_finalize.take();
                    drop(cached_block_to_finalize);

                    if let Some((cached_block_number, cached_block_hash)) =
                        maybe_cached_block_to_finalize
                    {
                        do_finalize_block(
                            &client,
                            cached_block_number,
                            cached_block_hash,
                            &subcoin_network_api,
                            substrate_sync_service.as_ref(),
                        );
                    }

                    finalizer_worker_is_busy.store(false, Ordering::SeqCst);
                }),
            );
        }
    }
}

fn do_finalize_block<Block, Client, Backend>(
    client: &Arc<Client>,
    confirmed_block_number: NumberFor<Block>,
    confirmed_block_hash: Block::Hash,
    subcoin_network_api: &Arc<dyn NetworkApi>,
    substrate_sync_service: Option<&Arc<SyncingService<Block>>>,
) where
    Block: BlockT,
    Client: HeaderBackend<Block> + Finalizer<Block, Backend>,
    Backend: sc_client_api::backend::Backend<Block>,
{
    let finalized_number = client.info().finalized_number;

    if confirmed_block_number <= finalized_number {
        return;
    }

    match client.finalize_block(confirmed_block_hash, None, true) {
        Ok(()) => {
            let is_major_syncing = subcoin_network_api.is_major_syncing()
                || substrate_sync_service
                    .map(|sync_service| sync_service.is_major_syncing())
                    .unwrap_or(false);

            // Only print the log when not major syncing to not clutter the logs.
            if !is_major_syncing {
                tracing::info!(
                    "✅ Successfully finalized block #{confirmed_block_number},{confirmed_block_hash}"
                );
            }
        }
        Err(err) => {
            tracing::warn!(
                ?err,
                ?finalized_number,
                "Failed to finalize block #{confirmed_block_number},{confirmed_block_hash}",
            );
        }
    }
}
