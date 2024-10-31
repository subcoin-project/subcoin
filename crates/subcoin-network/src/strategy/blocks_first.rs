use crate::block_downloader::BlockDownloader;
use crate::peer_store::PeerStore;
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

/// Download state.
#[derive(Debug, Clone)]
enum State {
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
    DownloadingInv(Range<u32>),
    /// Downloading blocks upon the completion of inventories download.
    DownloadingBlocks(Range<u32>),
    /// All blocks up to the target block have been successfully downloaded,
    /// the download process has been completed.
    Completed,
}

impl State {
    fn is_downloading_blocks(&self) -> bool {
        matches!(self, Self::DownloadingBlocks(..))
    }
}

/// Download blocks using the Blocks-First strategy.
#[derive(Clone)]
pub struct BlocksFirstStrategy<Block, Client> {
    client: Arc<Client>,
    peer_id: PeerId,
    /// The final block number we are targeting when the download is complete.
    target_block_number: u32,
    state: State,
    block_downloader: BlockDownloader,
    downloaded_blocks_count: usize,
    last_locator_start: u32,
    _phantom: PhantomData<Block>,
}

impl<Block, Client> BlocksFirstStrategy<Block, Client>
where
    Block: BlockT,
    Client: HeaderBackend<Block> + AuxStore,
{
    pub(crate) fn new(
        client: Arc<Client>,
        peer_id: PeerId,
        peer_best: u32,
        peer_store: Arc<dyn PeerStore>,
    ) -> (Self, SyncRequest) {
        let best_number = client.best_number();

        let mut blocks_first_sync = Self {
            peer_id,
            client,
            target_block_number: peer_best,
            state: State::Idle,
            block_downloader: BlockDownloader::new(peer_id, best_number, peer_store),
            downloaded_blocks_count: 0,
            last_locator_start: 0u32,
            _phantom: Default::default(),
        };

        let get_blocks_request = blocks_first_sync.prepare_blocks_request();

        (blocks_first_sync, get_blocks_request)
    }

    pub(crate) fn sync_status(&self) -> SyncStatus {
        if self.block_downloader.queue_status.is_overloaded()
            || (self.state.is_downloading_blocks()
                && self.block_downloader.missing_blocks.is_empty())
        {
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

    pub(crate) fn replaceable_sync_peer(&self) -> Option<PeerId> {
        (self.downloaded_blocks_count == 0).then_some(self.peer_id)
    }

    pub(crate) fn replace_sync_peer(&mut self, peer_id: PeerId, target_block_number: u32) {
        self.peer_id = peer_id;
        self.target_block_number = target_block_number;
        self.downloaded_blocks_count = 0;
        self.block_downloader.peer_id = peer_id;
    }

    pub(crate) fn update_peer_best(&mut self, peer_id: PeerId, peer_best: u32) {
        if self.peer_id == peer_id {
            self.target_block_number = peer_best;
        }
    }

    pub(crate) fn block_downloader(&mut self) -> &mut BlockDownloader {
        &mut self.block_downloader
    }

    pub(crate) fn on_tick(&mut self) -> SyncAction {
        if matches!(self.state, State::Restarting) {
            return self.blocks_request_action();
        }

        if self.block_downloader.queue_status.is_overloaded() {
            // If the queue was overloaded, evalute the current queue status again.
            let is_ready = self
                .block_downloader
                .evaluate_queue_status(self.client.best_number())
                .is_ready();

            if is_ready {
                if matches!(self.state, State::DownloadingBlocks(..)) {
                    if self.block_downloader.missing_blocks.is_empty() {
                        return self.blocks_request_action();
                    } else if self.block_downloader.requested_blocks.is_empty() {
                        return self.block_downloader.schedule_next_download_batch();
                    }
                } else {
                    return self.blocks_request_action();
                }
            } else {
                return SyncAction::None;
            }
        } else if matches!(self.state, State::DownloadingBlocks(..)) {
            // If the queue was not overloaded, but we are in the downloading blocks mode,
            // see whether there are more blocks to download.
            let is_ready = self
                .block_downloader
                .evaluate_queue_status(self.client.best_number())
                .is_ready();

            if is_ready {
                if self.block_downloader.missing_blocks.is_empty() {
                    return self.blocks_request_action();
                } else if self.block_downloader.requested_blocks.is_empty() {
                    return self.block_downloader.schedule_next_download_batch();
                }
            } else {
                return SyncAction::None;
            }
        }

        if self.client.best_number() == self.target_block_number {
            self.state = State::Completed;
            return SyncAction::SetIdle;
        }

        if let Some(stalled_peer) = self.block_downloader.has_stalled() {
            return SyncAction::RestartSyncWithStalledPeer(stalled_peer);
        }

        SyncAction::None
    }

    pub(crate) fn restart(&mut self, new_peer: PeerId, peer_best: u32) {
        self.peer_id = new_peer;
        self.downloaded_blocks_count = 0;
        self.target_block_number = peer_best;
        self.last_locator_start = 0u32;
        self.block_downloader.reset();
        self.state = State::Restarting;
        self.block_downloader.peer_id = new_peer;
    }

    // Handle `inv` message.
    //
    // NOTE: `inv` can be received unsolicited as an announcement of a new block,
    // or in reply to `getblocks`.
    pub(crate) fn on_inv(&mut self, inventories: Vec<Inventory>, from: PeerId) -> SyncAction {
        if from != self.peer_id {
            tracing::debug!(?from, current_sync_peer = ?self.peer_id, "Recv unexpected {} inventories", inventories.len());
            tracing::debug!(?from, "TODO: inventories: {inventories:?}");
            return SyncAction::None;
        }

        if inventories
            .iter()
            .filter(|inv| matches!(inv, Inventory::Block(_)))
            .count()
            > MAX_GET_BLOCKS_RESPONSE as usize
        {
            tracing::warn!(
                ?from,
                "Received inv with more than {MAX_GET_BLOCKS_RESPONSE} block entries"
            );
            self.state = State::Disconnecting;
            return SyncAction::Disconnect(self.peer_id, Error::InvHasTooManyBlockItems);
        }

        let range = match &self.state {
            State::DownloadingInv(range) => range,
            state => {
                tracing::debug!("Recv inventories in state {state:?}");
                return SyncAction::None;
            }
        };

        tracing::debug!("Processing {} inventories", inventories.len());

        let mut missing_blocks = Vec::new();

        for inv in inventories {
            match inv {
                Inventory::Block(block_hash) => {
                    if !self.client.block_exists(block_hash)
                        && self.block_downloader.is_unknown_block(block_hash)
                    {
                        missing_blocks.push(block_hash);
                    }
                }
                Inventory::WitnessBlock(_block_hash) => {}
                Inventory::Transaction(_txid) => {}
                Inventory::WitnessTransaction(_txid) => {}
                _ => {}
            }
        }

        if missing_blocks.is_empty() {
            return self.blocks_request_action();
        }

        tracing::debug!("Found {} blocks missing", missing_blocks.len());

        self.block_downloader.set_missing_blocks(missing_blocks);

        self.state = State::DownloadingBlocks(range.clone());

        self.block_downloader.schedule_next_download_batch()
    }

    pub(crate) fn on_block(&mut self, block: BitcoinBlock, from: PeerId) -> SyncAction {
        if from != self.peer_id {
            tracing::debug!(?from, current_sync_peer = ?self.peer_id, "Recv unexpected block #{}", block.block_hash());
            return SyncAction::None;
        }

        let last_get_blocks_target = match &self.state {
            State::DownloadingBlocks(range) => range.end - 1,
            state => {
                if let Ok(height) = block.bip34_block_height() {
                    tracing::debug!(
                        ?state,
                        current_sync_peer = ?self.peer_id,
                        "Recv unexpected block #{height},{} from {from:?}",
                        block.block_hash()
                    );
                } else {
                    tracing::debug!(
                        ?state,
                        current_sync_peer = ?self.peer_id,
                        "Recv unexpected block {} from {from:?}",
                        block.block_hash()
                    );
                }

                return SyncAction::None;
            }
        };

        let block_hash = block.block_hash();

        if self.block_downloader.block_exists(block_hash) {
            tracing::debug!(?block_hash, "Received duplicate block");
            return SyncAction::None;
        }

        let parent_block_hash = block.header.prev_blockhash;

        let maybe_parent =
            if let Some(number) = self.block_downloader.block_number(parent_block_hash) {
                Some(number)
            } else {
                self.client.block_number(parent_block_hash)
            };

        let receive_requested_block = self.block_downloader.on_block_response(block_hash);

        if let Some(parent_block_number) = maybe_parent {
            let block_number = parent_block_number + 1;

            tracing::trace!("Add pending block #{block_number},{block_hash}");

            self.block_downloader
                .add_block(block_number, block_hash, block, from);

            self.downloaded_blocks_count += 1;

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
                        self.state = State::Completed;
                        SyncAction::SetIdle
                    } else {
                        self.state = State::Disconnecting;
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
                        .block_downloader
                        .evaluate_queue_status(best_number)
                        .is_overloaded()
                    {
                        return SyncAction::None;
                    }

                    let best_queued_number = self.block_downloader.best_queued_number;

                    let end = self
                        .target_block_number
                        .min(best_queued_number + MAX_GET_BLOCKS_RESPONSE);

                    self.state = State::DownloadingInv(best_queued_number + 1..end + 1);
                    self.block_downloader.clear_missing_blocks();

                    let BlockLocator { locator_hashes, .. } = self
                        .client
                        .block_locator(Some(best_queued_number), |height: u32| {
                            self.block_downloader.queued_blocks.block_hash(height)
                        });

                    tracing::debug!(
                        best_number,
                        best_queued_number,
                        "Last `getblocks` finished, fetching more blocks",
                    );

                    SyncAction::Request(SyncRequest::Blocks(LocatorRequest {
                        locator_hashes,
                        stop_hash: BlockHash::all_zeros(),
                        to: self.peer_id,
                    }))
                }
            }
        } else {
            if self
                .block_downloader
                .orphan_blocks_pool
                .contains_unknown_block(&block_hash)
            {
                tracing::debug!("Received orphan block {block_hash} again");
                return SyncAction::None;
            }

            // Orphan blocks
            if receive_requested_block {
                self.block_downloader.add_orphan_block(block_hash, block);
                SyncAction::None
            } else {
                tracing::debug!(
                    best_number = ?self.client.info().best_number,
                    best_queued_number = self.block_downloader.best_queued_number,
                    parent_block_hash = ?block.header.prev_blockhash,
                    "Received unrequested orphan block {block_hash}"
                );

                self.block_downloader.add_unknown_block(block_hash, block);

                if self
                    .block_downloader
                    .evaluate_queue_status(self.client.best_number())
                    .is_overloaded()
                {
                    SyncAction::None
                } else {
                    self.blocks_request_action()
                }
            }
        }
    }

    #[inline]
    fn blocks_request_action(&mut self) -> SyncAction {
        SyncAction::Request(self.prepare_blocks_request())
    }

    fn prepare_blocks_request(&mut self) -> SyncRequest {
        let BlockLocator {
            latest_block,
            locator_hashes,
        } = self.block_locator();

        let end = self
            .target_block_number
            .min(latest_block + MAX_GET_BLOCKS_RESPONSE);

        self.state = State::DownloadingInv(latest_block + 1..end + 1);

        tracing::debug!(
            latest_block,
            best_queued_number = self.block_downloader.best_queued_number,
            ?locator_hashes,
            "Requesting new blocks",
        );

        SyncRequest::Blocks(LocatorRequest {
            locator_hashes,
            stop_hash: BlockHash::all_zeros(),
            to: self.peer_id,
        })
    }

    fn block_locator(&mut self) -> BlockLocator {
        let from = self
            .client
            .best_number()
            .max(self.block_downloader.best_queued_number);

        if from > 0 && self.last_locator_start == from {
            return BlockLocator::empty();
        }

        self.last_locator_start = from;
        self.client.block_locator(Some(from), |height: u32| {
            self.block_downloader.queued_blocks.block_hash(height)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peer_store::NoPeerStore;
    use subcoin_test_service::block_data;

    #[test]
    fn duplicate_block_announcement_should_not_be_downloaded_again() {
        sp_tracing::try_init_simple();

        let runtime = tokio::runtime::Runtime::new().expect("Create tokio runtime");

        let subcoin_service::NodeComponents { client, .. } =
            subcoin_test_service::new_test_node(runtime.handle().clone())
                .expect("Create test node");

        let peer_id: PeerId = "0.0.0.0:0".parse().unwrap();
        let (mut strategy, _initial_request) =
            BlocksFirstStrategy::new(client, peer_id, 800000, Arc::new(NoPeerStore));

        let block = block_data()[3].clone();
        let block_hash = block.block_hash();

        // Request the block when peer sent us a block announcement via inv.
        let sync_action = strategy.on_inv(vec![Inventory::Block(block_hash)], peer_id);

        match sync_action {
            SyncAction::Request(SyncRequest::Data(blocks_request, _)) => {
                assert_eq!(blocks_request, vec![Inventory::Block(block_hash)])
            }
            action => panic!("Should request block data but got: {action:?}"),
        }

        let parent_hash = block.header.prev_blockhash;
        assert!(!strategy
            .block_downloader
            .orphan_blocks_pool
            .contains_orphan_block(&parent_hash));
        assert!(!strategy
            .block_downloader
            .orphan_blocks_pool
            .block_exists(&block_hash));

        // Block received, but the parent is still missing, we add this block to the orphan blocks
        // pool.
        strategy.on_block(block, peer_id);
        assert!(strategy
            .block_downloader
            .orphan_blocks_pool
            .contains_orphan_block(&parent_hash));
        assert!(strategy
            .block_downloader
            .orphan_blocks_pool
            .block_exists(&block_hash));

        // The same block announcement was received, but we don't download it again.
        let sync_action = strategy.on_inv(vec![Inventory::Block(block_hash)], peer_id);

        assert!(
            matches!(sync_action, SyncAction::None),
            "Should do nothing but got: {sync_action:?}"
        );
    }
}
