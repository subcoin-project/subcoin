use crate::peer_store::PeerStore;
use crate::sync::block_downloader::BlockDownloader;
use crate::sync::{LocatorRequest, SyncAction};
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
    /// Actively downloading new block inventories in the specified range.
    FetchingInventory(Range<u32>),
    /// Downloading full blocks upon the completion of inventories download.
    FetchingBlockData(Range<u32>),
    /// All blocks up to the target block have been successfully downloaded,
    /// the download process has been completed.
    Completed,
}

impl State {
    fn is_fetching_block_data(&self) -> bool {
        matches!(self, Self::FetchingBlockData(..))
    }
}

/// Responsible for sending `GetBlocks` requests to retrieve block inventories.
#[derive(Clone)]
struct InventoryRequester<Block, Client> {
    client: Arc<Client>,
    peer_id: PeerId,
    target_block_number: u32,
    last_locator_request_start: u32,
    _phantom: PhantomData<Block>,
}

enum InvRequestOutcome {
    /// Indicates a redundant request to avoid unnecessary inventory fetching.
    RepeatedRequest,
    /// New `GetBlocks` request with specified locator hashes and range.
    NewGetBlocks {
        payload: LocatorRequest,
        range: Range<u32>,
    },
}

impl<Block, Client> InventoryRequester<Block, Client>
where
    Block: BlockT,
    Client: HeaderBackend<Block> + AuxStore,
{
    fn next_inventory_request(&mut self, block_downloader: &BlockDownloader) -> InvRequestOutcome {
        let from = self
            .client
            .best_number()
            .max(block_downloader.best_queued_number);

        if from > 0 && self.last_locator_request_start == from {
            return InvRequestOutcome::RepeatedRequest;
        }

        self.last_locator_request_start = from;

        let BlockLocator {
            latest_block,
            locator_hashes,
        } = self.client.block_locator(Some(from), |height: u32| {
            block_downloader.queued_blocks.block_hash(height)
        });

        let end = self
            .target_block_number
            .min(latest_block + MAX_GET_BLOCKS_RESPONSE);

        tracing::debug!(
            latest_block,
            best_queued_number = block_downloader.best_queued_number,
            "Requesting new block inventories",
        );

        let payload = LocatorRequest {
            locator_hashes,
            stop_hash: BlockHash::all_zeros(),
            to: self.peer_id,
        };

        InvRequestOutcome::NewGetBlocks {
            payload,
            range: latest_block + 1..end + 1,
        }
    }
}

/// Download blocks using the Blocks-First strategy.
#[derive(Clone)]
pub struct BlocksFirstStrategy<Block, Client> {
    state: State,
    client: Arc<Client>,
    peer_id: PeerId,
    /// The final block number we are targeting when the download is complete.
    target_block_number: u32,
    block_downloader: BlockDownloader,
    inv_requester: InventoryRequester<Block, Client>,
}

impl<Block, Client> BlocksFirstStrategy<Block, Client>
where
    Block: BlockT,
    Client: HeaderBackend<Block> + AuxStore,
{
    pub(crate) fn new(
        client: Arc<Client>,
        peer_id: PeerId,
        target_block_number: u32,
        peer_store: Arc<dyn PeerStore>,
    ) -> (Self, SyncAction) {
        let best_number = client.best_number();

        let inv_requester = InventoryRequester {
            client: client.clone(),
            peer_id,
            target_block_number,
            last_locator_request_start: 0u32,
            _phantom: Default::default(),
        };

        let mut blocks_first_sync = Self {
            peer_id,
            client,
            target_block_number,
            state: State::Idle,
            inv_requester,
            block_downloader: BlockDownloader::new(peer_id, best_number, peer_store),
        };

        let outcome = blocks_first_sync
            .inv_requester
            .next_inventory_request(&blocks_first_sync.block_downloader);
        let sync_action = blocks_first_sync.process_inv_request_outcome(outcome);

        (blocks_first_sync, sync_action)
    }

    pub(crate) fn sync_status(&self) -> SyncStatus {
        if self.block_downloader.queue_status.is_overloaded()
            || (self.state.is_fetching_block_data()
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

    pub(crate) fn can_swap_sync_peer(&self) -> Option<PeerId> {
        (self.block_downloader.downloaded_blocks_count == 0).then_some(self.peer_id)
    }

    pub(crate) fn swap_sync_peer(&mut self, peer_id: PeerId, target_block_number: u32) {
        self.peer_id = peer_id;
        self.target_block_number = target_block_number;
        self.block_downloader.peer_id = peer_id;
        self.block_downloader.downloaded_blocks_count = 0;
    }

    pub(crate) fn set_peer_best(&mut self, peer_id: PeerId, peer_best: u32) {
        if self.peer_id == peer_id {
            self.target_block_number = peer_best;
        }
    }

    pub(crate) fn block_downloader(&mut self) -> &mut BlockDownloader {
        &mut self.block_downloader
    }

    pub(crate) fn on_tick(&mut self) -> SyncAction {
        if matches!(self.state, State::Restarting) {
            return self.inventory_request_action();
        }

        if self.client.best_number() == self.target_block_number {
            self.state = State::Completed;
            return SyncAction::SetIdle;
        }

        if self.block_downloader.queue_status.is_overloaded() {
            // If the queue was overloaded, evalute the current queue status again.
            let is_ready = self
                .block_downloader
                .evaluate_queue_status(self.client.best_number())
                .is_ready();

            if is_ready {
                if matches!(self.state, State::FetchingBlockData(..)) {
                    if self.block_downloader.missing_blocks.is_empty() {
                        return self.inventory_request_action();
                    } else if self.block_downloader.requested_blocks.is_empty() {
                        return self.block_downloader.schedule_next_download_batch();
                    }
                } else {
                    return self.inventory_request_action();
                }
            } else {
                return SyncAction::None;
            }
        }

        if let Some(stalled_peer) = self.block_downloader.has_stalled() {
            return SyncAction::RestartSyncWithStalledPeer(stalled_peer);
        }

        if matches!(self.state, State::FetchingBlockData(..)) {
            // If the queue was not overloaded, but we are in the downloading blocks mode,
            // see whether there are pending blocks in the block_downloader.
            let is_ready = self
                .block_downloader
                .evaluate_queue_status(self.client.best_number())
                .is_ready();

            if is_ready && self.block_downloader.requested_blocks.is_empty() {
                if self.block_downloader.missing_blocks.is_empty() {
                    return self.inventory_request_action();
                } else {
                    return self.block_downloader.schedule_next_download_batch();
                }
            }
        }

        SyncAction::None
    }

    pub(crate) fn restart(&mut self, new_peer: PeerId, target_block_number: u32) {
        self.state = State::Restarting;
        self.peer_id = new_peer;
        self.target_block_number = target_block_number;
        self.block_downloader.restart(new_peer);
        self.inv_requester.peer_id = new_peer;
        self.inv_requester.last_locator_request_start = 0u32;
    }

    // Handle `inv` message.
    //
    // NOTE: `inv` can be received unsolicited as an announcement of a new block,
    // or in reply to `getblocks`.
    pub(crate) fn on_inv(&mut self, inventories: Vec<Inventory>, from: PeerId) -> SyncAction {
        if from != self.peer_id {
            // TODO: handle transaction inventories
            // tracing::debug!(?from, current_sync_peer = ?self.peer_id, "Recv unexpected {} inventories", inventories.len());
            // tracing::debug!(?from, "TODO: inventories: {inventories:?}");
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
            return SyncAction::DisconnectPeer(self.peer_id, Error::InvHasTooManyBlockItems);
        }

        let range = match &self.state {
            State::FetchingInventory(range) => range,
            state => {
                tracing::debug!(?state, "Ignored inventories {inventories:?}");
                return SyncAction::None;
            }
        };

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
            return self.inventory_request_action();
        }

        self.state = State::FetchingBlockData(range.clone());
        self.block_downloader.set_missing_blocks(missing_blocks);
        self.block_downloader.schedule_next_download_batch()
    }

    pub(crate) fn on_block(&mut self, block: BitcoinBlock, from: PeerId) -> SyncAction {
        if from != self.peer_id {
            tracing::debug!(?from, current_sync_peer = ?self.peer_id, "Recv unexpected block #{}", block.block_hash());
            return SyncAction::None;
        }

        let last_get_blocks_target = match &self.state {
            State::FetchingBlockData(range) => range.end - 1,
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

        let maybe_parent = self
            .block_downloader
            .block_number(parent_block_hash)
            .or_else(|| self.client.block_number(parent_block_hash));

        let receive_requested_block = self.block_downloader.on_block_response(block_hash);

        if let Some(parent_block_number) = maybe_parent {
            let block_number = parent_block_number + 1;

            tracing::trace!("Add pending block #{block_number},{block_hash}");

            self.block_downloader
                .add_block(block_number, block_hash, block, from);

            self.schedule_block_download(block_number, block_hash, last_get_blocks_target)
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
                    self.inventory_request_action()
                }
            }
        }
    }

    /// Request more blocks if there are remaining blocks in the current block list, otherwise
    /// request a new list of missing blocks for downloading.
    fn schedule_block_download(
        &mut self,
        block_number: u32,
        block_hash: BlockHash,
        last_get_blocks_target: u32,
    ) -> SyncAction {
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
                        "Received block #{block_number},{block_hash} higher than the target #{}",
                        self.target_block_number
                    );
                    self.state = State::Completed;
                    SyncAction::SetIdle
                } else {
                    SyncAction::None
                }
            }
            CmpOrdering::Equal => {
                // No more new blocks request as there are enough ongoing blocks in the queue.
                if self
                    .block_downloader
                    .evaluate_queue_status(self.client.best_number())
                    .is_overloaded()
                {
                    return SyncAction::None;
                }

                if self.block_downloader.missing_blocks.is_empty() {
                    self.inventory_request_action()
                } else {
                    self.block_downloader.schedule_next_download_batch()
                }
            }
        }
    }

    #[inline]
    fn inventory_request_action(&mut self) -> SyncAction {
        let inv_request_outcome = self
            .inv_requester
            .next_inventory_request(&self.block_downloader);
        self.process_inv_request_outcome(inv_request_outcome)
    }

    fn process_inv_request_outcome(
        &mut self,
        inv_request_outcome: InvRequestOutcome,
    ) -> SyncAction {
        match inv_request_outcome {
            InvRequestOutcome::RepeatedRequest => SyncAction::None,
            InvRequestOutcome::NewGetBlocks { payload, range } => {
                self.state = State::FetchingInventory(range);
                SyncAction::get_inventory(payload)
            }
        }
    }
}
