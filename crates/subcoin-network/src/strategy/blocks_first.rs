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
    /// All blocks up to the target block have been successfully downloaded,
    /// the download process has been completed.
    Completed,
}

/// Download blocks using the Blocks-First strategy.
#[derive(Clone)]
pub struct BlocksFirstDownloader<Block, Client> {
    client: Arc<Client>,
    peer_id: PeerId,
    /// The final block number we are targeting when the download is complete.
    target_block_number: u32,
    state: State,
    block_downloader: BlockDownloader,
    downloaded_blocks_count: usize,
    last_locator_start: u32,
    pending_block_requests: Vec<Inventory>,
    /// Number of blocks' data requested but not yet received.
    inflight_blocks_count: usize,
    _phantom: PhantomData<Block>,
}

impl<Block, Client> BlocksFirstDownloader<Block, Client>
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
        let mut blocks_first_sync = Self {
            peer_id,
            client,
            target_block_number: peer_best,
            state: State::Idle,
            block_downloader: BlockDownloader::new(peer_store),
            downloaded_blocks_count: 0,
            last_locator_start: 0u32,
            pending_block_requests: Vec::new(),
            inflight_blocks_count: 0,
            _phantom: Default::default(),
        };

        let get_blocks_request = blocks_first_sync.prepare_blocks_request();

        (blocks_first_sync, get_blocks_request)
    }

    pub(crate) fn sync_status(&self) -> SyncStatus {
        if self.block_downloader.queue_status.is_overloaded() {
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
    }

    pub(crate) fn block_downloader(&mut self) -> &mut BlockDownloader {
        &mut self.block_downloader
    }

    pub(crate) fn on_tick(&mut self) -> SyncAction {
        if matches!(self.state, State::Restarting) {
            return SyncAction::Request(self.prepare_blocks_request());
        }

        let best_number = self.client.best_number();

        if self.block_downloader.queue_status.is_overloaded() {
            let is_ready = self
                .block_downloader
                .evaluate_queue_status(best_number)
                .is_ready();
            if is_ready {
                return SyncAction::Request(self.prepare_blocks_request());
            } else {
                return SyncAction::None;
            }
        }

        if !self.pending_block_requests.is_empty() && self.inflight_blocks_count == 0 {
            let block_data_request = std::mem::take(&mut self.pending_block_requests);
            return self.truncate_and_prepare_block_data_request(block_data_request);
        }

        if best_number == self.target_block_number {
            self.state = State::Completed;
            return SyncAction::SetIdle;
        }

        if self.block_downloader.is_stalled(self.peer_id) {
            return SyncAction::RestartSyncWithStalledPeer(self.peer_id);
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
        self.pending_block_requests.clear();
        self.inflight_blocks_count = 0;
    }

    // Handle `inv` message.
    //
    // NOTE: `inv` can be received unsolicited as an announcement of a new block,
    // or in reply to `getblocks`.
    pub(crate) fn on_inv(&mut self, inventories: Vec<Inventory>, from: PeerId) -> SyncAction {
        if from != self.peer_id {
            tracing::debug!(?from, current_sync_peer = ?self.peer_id, "Recv unexpected {} inventories", inventories.len());
            if inventories.len() == 1 {
                tracing::debug!(
                    ?from,
                    "TODO: block announcement, inventories: {inventories:?}"
                );
            }
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

        let mut block_data_request = Vec::new();

        for inv in inventories {
            match inv {
                Inventory::Block(block_hash) => {
                    if !self.client.block_exists(block_hash)
                        && self.block_downloader.is_unknown_block(block_hash)
                    {
                        block_data_request.push(Inventory::Block(block_hash));
                        self.block_downloader.requested_blocks.insert(block_hash);
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

        self.truncate_and_prepare_block_data_request(block_data_request)
    }

    /// Prepares and truncates the block data request to avoid overly large requests.
    ///
    /// To minimize potential failures and latency, this function splits large `Inventory` lists
    /// into smaller requests if they exceed a predefined maximum size. If truncation is necessary,
    /// the remaining items are stored in `pending_block_requests` for future processing.
    ///
    /// The number of blocks in the current request is stored in `inflight_blocks_count` to track
    /// the ongoing download progress.
    ///
    /// # Parameters
    ///
    /// - `block_data_request`: A `Vec` of `Inventory` items representing blocks to be downloaded
    ///   from the peer.
    ///
    /// # Returns
    ///
    /// A `SyncAction` containing a `SyncRequest::Data` with the truncated list of blocks to
    /// request from the peer.
    fn truncate_and_prepare_block_data_request(
        &mut self,
        mut block_data_request: Vec<Inventory>,
    ) -> SyncAction {
        let max_request_size = self.max_block_data_request_size();

        if block_data_request.len() > max_request_size {
            self.pending_block_requests = block_data_request.split_off(max_request_size);
        }

        self.inflight_blocks_count = block_data_request.len();

        tracing::debug!(
            from = ?self.peer_id,
            pending_block_data_request = self.pending_block_requests.len(),
            "📦 Downloading {} blocks",
            self.inflight_blocks_count,
        );

        SyncAction::Request(SyncRequest::Data(block_data_request, self.peer_id))
    }

    pub(crate) fn on_block(&mut self, block: BitcoinBlock, from: PeerId) -> SyncAction {
        if from != self.peer_id {
            tracing::debug!(?from, current_sync_peer = ?self.peer_id, "Recv unexpected block #{}", block.block_hash());
            return SyncAction::None;
        }

        let last_get_blocks_target = match &self.state {
            State::DownloadingInv(range) => range.end - 1,
            state => {
                tracing::debug!(
                    ?state,
                    ?from,
                    current_sync_peer = ?self.peer_id,
                    "Recv unexpected block {}",
                    block.block_hash()
                );
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

            self.inflight_blocks_count -= 1;
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

        self.state = State::DownloadingInv(latest_block + 1..end + 1);

        tracing::debug!(
            latest_block,
            best_queued_number = self.block_downloader.best_queued_number,
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

    fn max_block_data_request_size(&self) -> usize {
        let best_known = self
            .client
            .best_number()
            .max(self.block_downloader.best_queued_number);

        match best_known {
            0..=99_999 => 500,
            100_000..=199_999 => 256,
            200_000..=299_999 => 128,
            300_000..=399_999 => 64,
            400_000..=499_999 => 32,
            500_000..=599_999 => 16,
            600_000..=699_999 => 8,
            700_000..=799_999 => 4,
            _ => 2,
        }
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
        let (mut downloader, _initial_request) =
            BlocksFirstDownloader::new(client, peer_id, 800000, Arc::new(NoPeerStore));

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
            .block_downloader
            .orphan_blocks_pool
            .contains_orphan_block(&parent_hash));
        assert!(!downloader
            .block_downloader
            .orphan_blocks_pool
            .block_exists(&block_hash));

        // Block received, but the parent is still missing, we add this block to the orphan blocks
        // pool.
        downloader.on_block(block, peer_id);
        assert!(downloader
            .block_downloader
            .orphan_blocks_pool
            .contains_orphan_block(&parent_hash));
        assert!(downloader
            .block_downloader
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
