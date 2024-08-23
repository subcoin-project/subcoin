use futures::StreamExt;
use sc_client_api::{BlockchainEvents, Finalizer, HeaderBackend};
use sc_network_sync::SyncingService;
use sc_service::SpawnTaskHandle;
use sp_consensus::SyncOracle;
use sp_runtime::traits::{Block as BlockT, CheckedSub};
use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// This struct is responsible for finalizing blocks with enough confirmations.
pub struct SubcoinFinalizer<Block: BlockT, Client, Backend> {
    client: Arc<Client>,
    spawn_handle: SpawnTaskHandle,
    confirmation_depth: u32,
    major_sync_confirmation_depth: u32,
    subcoin_networking_is_major_syncing: Arc<AtomicBool>,
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
        major_sync_confirmation_depth: u32,
        subcoin_networking_is_major_syncing: Arc<AtomicBool>,
        substrate_sync_service: Option<Arc<SyncingService<Block>>>,
    ) -> Self {
        Self {
            client,
            spawn_handle,
            confirmation_depth,
            major_sync_confirmation_depth,
            subcoin_networking_is_major_syncing,
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
            major_sync_confirmation_depth,
            subcoin_networking_is_major_syncing,
            substrate_sync_service,
            _phantom,
        } = self;

        // Use `every_import_notification_stream()` so that we can receive the notifications even when
        // the Substrate network is major syncing.
        let mut block_import_stream = client.every_import_notification_stream();

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

            if subcoin_networking_is_major_syncing.load(Ordering::Relaxed) {
                // During the major sync of Subcoin networking, we choose to finalize every `major_sync_confirmation_depth`
                // block to avoid race conditions:
                // >Safety violation: attempted to revert finalized block...
                if confirmed_block_number < finalized_number + major_sync_confirmation_depth.into()
                {
                    continue;
                }
            }

            if let Some(sync_service) = substrate_sync_service.as_ref() {
                // State sync relies on the finalized block notification to progress
                // Substrate chain sync component relies on the finalized block notification to
                // initiate the state sync, do not attempt to finalize the block when the queued blocks
                // are not empty, so that the state sync can be started when the last finalized block
                // notification is sent.
                if sync_service.is_major_syncing()
                    && sync_service.num_queued_blocks().await.unwrap_or(0) > 0
                {
                    continue;
                }
            }

            let block_to_finalize = client
                .hash(confirmed_block_number)
                .ok()
                .flatten()
                .expect("Confirmed block must be available; qed");

            let client = client.clone();
            let subcoin_networking_is_major_syncing = subcoin_networking_is_major_syncing.clone();
            let substrate_sync_service = substrate_sync_service.clone();

            spawn_handle.spawn(
                "finalize-block",
                None,
                Box::pin(async move {
                    if confirmed_block_number <= client.info().finalized_number {
                        return;
                    }

                    match client.finalize_block(block_to_finalize, None, true) {
                        Ok(()) => {
                            let is_major_syncing = subcoin_networking_is_major_syncing.load(Ordering::Relaxed)
                                || substrate_sync_service
                                    .map(|sync_service| sync_service.is_major_syncing())
                                    .unwrap_or(false);

                            // Only print the log when not major syncing to not clutter the logs.
                            if !is_major_syncing {
                                tracing::info!("✅ Successfully finalized block: {block_to_finalize}");
                            }
                        }
                        Err(err) => {
                            tracing::warn!(
                                ?err,
                                ?finalized_number,
                                "Failed to finalize block #{confirmed_block_number},{block_to_finalize}",
                            );
                        }
                    }
                }),
            );
        }
    }
}
