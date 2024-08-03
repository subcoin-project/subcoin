use crate::block_downloader::BlockDownloadManager;
use crate::sync::{LocatorRequest, SyncAction, SyncRequest};
use crate::{Error, PeerId, SyncStatus};
use bitcoin::hashes::Hash;
use bitcoin::p2p::message_blockdata::Inventory;
use bitcoin::{Block as BitcoinBlock, BlockHash};
use sc_client_api::AuxStore;
use sp_blockchain::HeaderBackend;
use sp_runtime::traits::Block as BlockT;
use std::cmp::Ordering as CmpOrdering;
use std::marker::PhantomData;
use std::ops::Range;
use std::sync::Arc;
use subcoin_primitives::{BackendExt, BlockLocator, BlockLocatorProvider, ClientExt};

// Maximum number of block inventories in `inv`, as a response to `getblocks` request.
//
// Assuming our best is #0, the expected block response of upcoming `getblocks`
// request is [1, 500] when doing the initial sync.
const MAX_GET_BLOCKS_RESPONSE: u32 = 500;

#[derive(Debug, Clone)]
enum DownloadState {
    /// Downloading not started yet.
    Idle,
    /// Restarting the download process.
    Restarting,
    /// Peer misbehavior detected, will disconnect the peer shortly.
    Disconnecting,
    /// Actively downloading new blocks in the specified range, in batches.
    ///
    /// When downloading blocks, the system fetches them in batches of a fixed size,
    /// up to 500 blocks in specific. It only continues to download the next batch
    /// of blocks once the previous request succeeds.
    DownloadingNew(Range<u32>),
    /// All blocks up to the target block have been successfully downloaded,
    /// the download process has been completed.
    Completed,
}

/// Download blocks using the Blocks-First strategy.
#[derive(Debug, Clone)]
pub struct BlocksFirstDownloader<Block, Client> {
    client: Arc<Client>,
    peer_id: PeerId,
    /// The final block number we are targeting when the download is complete.
    target_block_number: u32,
    download_state: DownloadState,
    download_manager: BlockDownloadManager,
    last_locator_start: u32,
    _phantom: PhantomData<Block>,
}

impl<Block, Client> BlocksFirstDownloader<Block, Client>
where
    Block: BlockT,
    Client: HeaderBackend<Block> + AuxStore,
{
    pub(crate) fn new(client: Arc<Client>, peer_id: PeerId, peer_best: u32) -> (Self, SyncRequest) {
        let mut blocks_first_sync = Self {
            peer_id,
            client,
            target_block_number: peer_best,
            download_state: DownloadState::Idle,
            download_manager: BlockDownloadManager::new(),
            last_locator_start: 0u32,
            _phantom: Default::default(),
        };

        let get_blocks_request = blocks_first_sync.prepare_blocks_request();

        (blocks_first_sync, get_blocks_request)
    }

    pub(crate) fn sync_status(&self) -> SyncStatus {
        if self.download_manager.import_queue_is_overloaded {
            SyncStatus::Importing {
                target: self.target_block_number,
                peers: vec![self.peer_id],
            }
        } else {
            SyncStatus::Downloading {
                target: self.target_block_number,
                peers: vec![self.peer_id],
            }
        }
    }

    pub(crate) fn sync_peer(&self) -> PeerId {
        self.peer_id
    }

    pub(crate) fn update_sync_peer(&mut self, peer_id: PeerId, target_block_number: u32) {
        self.peer_id = peer_id;
        self.target_block_number = target_block_number;
    }

    pub(crate) fn download_manager(&mut self) -> &mut BlockDownloadManager {
        &mut self.download_manager
    }

    pub(crate) fn on_tick(&mut self) -> SyncAction {
        if matches!(self.download_state, DownloadState::Restarting) {
            return SyncAction::Request(self.prepare_blocks_request());
        }

        if self.download_manager.import_queue_is_overloaded {
            if self
                .download_manager
                .update_and_check_queue_status(self.client.best_number())
            {
                return SyncAction::None;
            } else {
                return SyncAction::Request(self.prepare_blocks_request());
            }
        }

        if self.download_manager.is_stalled() {
            return SyncAction::RestartSyncWithStalledPeer(self.peer_id);
        }

        SyncAction::None
    }

    pub(crate) fn restart(&mut self, new_peer: PeerId, peer_best: u32) {
        self.peer_id = new_peer;
        self.target_block_number = peer_best;
        self.last_locator_start = 0u32;
        self.download_manager.reset();
        self.download_state = DownloadState::Restarting;
    }

    // Handle `inv` message.
    //
    // NOTE: `inv` can be received unsolicited as an announcement of a new block,
    // or in reply to `getblocks`.
    pub(crate) fn on_inv(&mut self, inventories: Vec<Inventory>, from: PeerId) -> SyncAction {
        // TODO: only handle the data from self.peer_id?

        let mut block_data_request = Vec::new();
        let mut block_inventories = 0;

        for inv in inventories {
            match inv {
                Inventory::Block(block_hash) => {
                    block_inventories += 1;

                    if block_inventories > MAX_GET_BLOCKS_RESPONSE {
                        tracing::warn!(
                            ?from,
                            "Received inv with more than {MAX_GET_BLOCKS_RESPONSE} block entries"
                        );
                        self.download_state = DownloadState::Disconnecting;
                        return SyncAction::Disconnect(self.peer_id, Error::TooManyBlockEntries);
                    }

                    if !self.client.block_exists(block_hash)
                        && self.download_manager.is_unknown_block(block_hash)
                    {
                        block_data_request.push(Inventory::Block(block_hash));
                        self.download_manager.requested_blocks.insert(block_hash);
                    }
                }
                Inventory::WitnessBlock(_block_hash) => {}
                Inventory::Transaction(_txid) => {}
                Inventory::WitnessTransaction(_txid) => {}
                _ => {}
            }
        }

        if block_data_request.is_empty() {
            return SyncAction::None;
        }

        tracing::debug!(
            from = ?self.peer_id,
            requested_blocks_count = self.download_manager.requested_blocks.len(),
            "📦 Downloading {} blocks", block_data_request.len(),
        );

        SyncAction::Request(SyncRequest::Data(block_data_request, self.peer_id))
    }

    pub(crate) fn on_block(&mut self, block: BitcoinBlock, from: PeerId) -> SyncAction {
        let last_get_blocks_target = match &self.download_state {
            DownloadState::DownloadingNew(range) => range.end - 1,
            state => {
                tracing::debug!(
                    ?state,
                    ?from,
                    current_sync_peer = ?self.peer_id,
                    "Received block {} while not in the mode of downloading blocks",
                    block.block_hash()
                );
                return SyncAction::None;
            }
        };

        let block_hash = block.block_hash();

        if self.download_manager.block_exists(block_hash) {
            tracing::debug!(?block_hash, "Received duplicate block");
            return SyncAction::None;
        }

        let parent_block_hash = block.header.prev_blockhash;

        let maybe_parent =
            if let Some(number) = self.download_manager.block_number(parent_block_hash) {
                Some(number)
            } else {
                self.client.block_number(parent_block_hash)
            };

        let receive_requested_block = self.download_manager.on_block_response(block_hash);

        if let Some(parent_block_number) = maybe_parent {
            let block_number = parent_block_number + 1;

            tracing::trace!("Add pending block #{block_number},{block_hash}");

            self.download_manager
                .add_block(block_number, block_hash, block);

            match block_number.cmp(&last_get_blocks_target) {
                CmpOrdering::Less => {
                    // The last `getblocks` request is not yet finished, waiting for more blocks to come.
                    SyncAction::None
                }
                CmpOrdering::Greater => {
                    if block_number >= self.target_block_number {
                        // Peer may send us higher blocks than our previous known peer_best when the chain grows.
                        tracing::debug!(
                            target_block_number = self.target_block_number,
                            "Received block #{block_number},{block_hash} higher than the target block"
                        );
                        self.download_state = DownloadState::Completed;
                        SyncAction::None
                    } else {
                        self.download_state = DownloadState::Disconnecting;
                        SyncAction::Disconnect(
                            self.peer_id,
                            Error::Other(format!(
                                "Received block#{block_number} higher than the target #{}",
                                self.target_block_number
                            )),
                        )
                    }
                }
                CmpOrdering::Equal => {
                    let best_number = self.client.best_number();

                    // No more new blocks request as there are enough ongoing blocks in the queue.
                    if self
                        .download_manager
                        .update_and_check_queue_status(best_number)
                    {
                        return SyncAction::None;
                    }

                    let best_queued_number = self.download_manager.best_queued_number;

                    tracing::debug!(
                        best_number,
                        best_queued_number,
                        "Last getblocks finished, requesting more blocks",
                    );

                    let end = self
                        .target_block_number
                        .min(best_queued_number + MAX_GET_BLOCKS_RESPONSE);

                    self.download_state =
                        DownloadState::DownloadingNew(best_queued_number + 1..end + 1);

                    let BlockLocator { locator_hashes, .. } = self
                        .client
                        .block_locator(Some(best_queued_number), |height: u32| {
                            self.download_manager.queued_blocks.block_hash(height)
                        });

                    // Last `getblocks` finished, fetch more blocks by sending a new `getblocks` request.
                    SyncAction::Request(SyncRequest::Blocks(LocatorRequest {
                        locator_hashes,
                        stop_hash: BlockHash::all_zeros(),
                        from: self.peer_id,
                    }))
                }
            }
        } else {
            if self
                .download_manager
                .orphan_blocks_pool
                .contains_unknown_block(&block_hash)
            {
                tracing::debug!("Received orphan block {block_hash} again");
                // New block might be announced using `block` message.
                // TODO: handle the orphan blocks once download completed.

                // FIXME: the tip block may be received too frequently?
                return SyncAction::None;
            }

            // Orphan blocks
            if receive_requested_block {
                self.download_manager.add_orphan_block(block_hash, block);
                SyncAction::None
            } else {
                tracing::debug!(
                    best_number = ?self.client.info().best_number,
                    best_queued_number = self.download_manager.best_queued_number,
                    parent_block_hash = ?block.header.prev_blockhash,
                    "Received unrequested orphan block {block_hash}"
                );

                self.download_manager.add_unknown_block(block_hash, block);

                if self
                    .download_manager
                    .update_and_check_queue_status(self.client.best_number())
                {
                    SyncAction::None
                } else {
                    // Request the parents of the orphan block.
                    SyncAction::Request(self.prepare_blocks_request())
                }
            }
        }
    }

    fn prepare_blocks_request(&mut self) -> SyncRequest {
        let BlockLocator {
            latest_block,
            locator_hashes,
        } = self.block_locator();

        let end = self
            .target_block_number
            .min(latest_block + MAX_GET_BLOCKS_RESPONSE);

        self.download_state = DownloadState::DownloadingNew(latest_block + 1..end + 1);

        tracing::debug!(
            latest_block,
            best_queued_number = self.download_manager.best_queued_number,
            "Requesting new blocks",
        );

        SyncRequest::Blocks(LocatorRequest {
            locator_hashes,
            stop_hash: BlockHash::all_zeros(),
            from: self.peer_id,
        })
    }

    fn block_locator(&mut self) -> BlockLocator {
        let from = self
            .client
            .best_number()
            .max(self.download_manager.best_queued_number);

        if from > 0 && self.last_locator_start == from {
            return BlockLocator::empty();
        }

        self.last_locator_start = from;
        self.client.block_locator(Some(from), |height: u32| {
            self.download_manager.queued_blocks.block_hash(height)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use subcoin_test_service::block_data;

    #[test]
    fn duplicate_block_announcement_should_not_be_downloaded_again() {
        sp_tracing::try_init_simple();

        let runtime = tokio::runtime::Runtime::new().expect("Create tokio runtime");

        let subcoin_service::NodeComponents { client, .. } =
            subcoin_test_service::new_test_node(runtime.handle().clone())
                .expect("Create test node");

        let peer_id: PeerId = "0.0.0.0:0".parse().unwrap();
        let (mut downloader, _initial_request) =
            BlocksFirstDownloader::new(client, peer_id, 800000);

        let block = block_data()[3].clone();
        let block_hash = block.block_hash();

        // Request the block when peer sent us a block announcement via inv.
        let sync_action = downloader.on_inv(vec![Inventory::Block(block_hash)], peer_id);

        match sync_action {
            SyncAction::Request(SyncRequest::Data(blocks_request, _)) => {
                assert_eq!(blocks_request, vec![Inventory::Block(block_hash)])
            }
            action => panic!("Should request block data but got: {action:?}"),
        }

        let parent_hash = block.header.prev_blockhash;
        assert!(!downloader
            .download_manager
            .orphan_blocks_pool
            .contains_orphan_block(&parent_hash));
        assert!(!downloader
            .download_manager
            .orphan_blocks_pool
            .block_exists(&block_hash));

        // Block received, but the parent is still missing, we add this block to the orphan blocks
        // pool.
        downloader.on_block(block, peer_id);
        assert!(downloader
            .download_manager
            .orphan_blocks_pool
            .contains_orphan_block(&parent_hash));
        assert!(downloader
            .download_manager
            .orphan_blocks_pool
            .block_exists(&block_hash));

        // The same block announcement was received, but we don't download it again.
        let sync_action = downloader.on_inv(vec![Inventory::Block(block_hash)], peer_id);

        assert!(
            matches!(sync_action, SyncAction::None),
            "Should do nothing but got: {sync_action:?}"
        );
    }
}
