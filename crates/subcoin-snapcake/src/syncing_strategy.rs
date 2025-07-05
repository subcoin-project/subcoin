use crate::state_sync_wrapper::StateSyncWrapper;
use codec::{Decode, Encode};
use futures::FutureExt;
use sc_client_api::{BlockBackend, HeaderBackend, ProofProvider};
use sc_consensus::{BlockImportError, BlockImportStatus};
use sc_network::config::{FullNetworkConfiguration, ProtocolId};
use sc_network::service::traits::RequestResponseConfig;
use sc_network::{NetworkBackend, PeerId, ProtocolName};
use sc_network_common::sync::message::{
    BlockAnnounce, BlockAttributes, BlockData, BlockRequest, Direction, FromBlock,
};
use sc_network_common::sync::SyncMode;
use sc_network_sync::block_relay_protocol::{BlockDownloader, BlockResponseError};
use sc_network_sync::service::network::NetworkServiceHandle;
use sc_network_sync::state_request_handler::StateRequestHandler;
use sc_network_sync::strategy::chain_sync::{ChainSync, ChainSyncMode};
use sc_network_sync::strategy::polkadot::PolkadotSyncingStrategyConfig;
use sc_network_sync::strategy::state::StateStrategy;
use sc_network_sync::strategy::warp::WarpSync;
use sc_network_sync::strategy::{StrategyKey, SyncingAction, SyncingStrategy};
use sc_network_sync::SyncStatus;
use sc_service::SpawnTaskHandle;
use sp_api::ProvideRuntimeApi;
use sp_blockchain::HeaderMetadata;
use sp_runtime::traits::{Block as BlockT, Header as HeaderT, NumberFor};
use std::any::Any;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use subcoin_primitives::runtime::SubcoinApi;
use subcoin_service::network_request_handler::{
    v1, VersionedNetworkRequest, VersionedNetworkResponse,
};

const LOG_TARGET: &str = "sync::snapcake";

const SUBCOIN_STRATEGY_KEY: StrategyKey = StrategyKey::new("Subcoin");

/// Maximum blocks per response.
const MAX_BLOCKS_IN_RESPONSE: usize = 128;

/// Corresponding `ChainSync` mode.
fn chain_sync_mode(sync_mode: SyncMode) -> ChainSyncMode {
    match sync_mode {
        SyncMode::Full => ChainSyncMode::Full,
        SyncMode::LightState {
            skip_proofs,
            storage_chain_mode,
        } => ChainSyncMode::LightState {
            skip_proofs,
            storage_chain_mode,
        },
        SyncMode::Warp => ChainSyncMode::Full,
    }
}

pub enum TargetBlock<Block: BlockT> {
    Hash(Block::Hash),
    Number(NumberFor<Block>),
    LastFinalized,
}

/// Build snapcake state syncing strategy.
#[allow(clippy::too_many_arguments)]
pub fn build_snapcake_syncing_strategy<Block, Client, Net>(
    protocol_id: ProtocolId,
    fork_id: Option<&str>,
    net_config: &mut FullNetworkConfiguration<Block, <Block as BlockT>::Hash, Net>,
    client: Arc<Client>,
    spawn_handle: &SpawnTaskHandle,
    block_downloader: Arc<dyn BlockDownloader<Block>>,
    skip_proof: bool,
    snapshot_dir: PathBuf,
    sync_target: TargetBlock<Block>,
) -> Result<Box<dyn SyncingStrategy<Block>>, sc_service::Error>
where
    Block: BlockT,
    Client: HeaderBackend<Block>
        + BlockBackend<Block>
        + HeaderMetadata<Block, Error = sp_blockchain::Error>
        + ProofProvider<Block>
        + ProvideRuntimeApi<Block>
        + Send
        + Sync
        + 'static,
    Client::Api: SubcoinApi<Block>,
    Net: NetworkBackend<Block, <Block as BlockT>::Hash>,
{
    let (state_request_protocol_config, state_request_protocol_name) = {
        let num_peer_hint = net_config.network_config.default_peers_set_num_full as usize
            + net_config
                .network_config
                .default_peers_set
                .reserved_nodes
                .len();
        // Allow both outgoing and incoming requests.
        let (handler, protocol_config) =
            StateRequestHandler::new::<Net>(&protocol_id, fork_id, client.clone(), num_peer_hint);
        let config_name = protocol_config.protocol_name().clone();

        spawn_handle.spawn("state-request-handler", Some("networking"), handler.run());
        (protocol_config, config_name)
    };
    net_config.add_request_response_protocol(state_request_protocol_config);

    let network_request_protocol_config = {
        // Allow outgoing requests only.
        let (_handler, protocol_config) =
            subcoin_service::network_request_handler::NetworkRequestHandler::new::<Net>(
                &protocol_id,
                fork_id,
                client.clone(),
                100,
            );
        protocol_config
    };

    let subcoin_network_request_protocol_name =
        network_request_protocol_config.protocol_name().clone();
    net_config.add_request_response_protocol(network_request_protocol_config);

    let syncing_config = PolkadotSyncingStrategyConfig {
        mode: net_config.network_config.sync_mode,
        max_parallel_downloads: net_config.network_config.max_parallel_downloads,
        max_blocks_per_request: net_config.network_config.max_blocks_per_request,
        metrics_registry: None,
        state_request_protocol_name,
        block_downloader,
        min_peers_to_start_warp_sync: None
    };

    Ok(Box::new(SnapcakeSyncingStrategy::new(
        syncing_config,
        client,
        skip_proof,
        snapshot_dir,
        sync_target,
        subcoin_network_request_protocol_name,
    )?))
}

/// Proxy to specific syncing strategies used in Polkadot.
pub struct SnapcakeSyncingStrategy<B: BlockT, Client> {
    /// Initial syncing configuration.
    config: PolkadotSyncingStrategyConfig<B>,
    /// Client used by syncing strategies.
    client: Arc<Client>,
    /// State strategy.
    state: Option<StateStrategy<B>>,
    /// `ChainSync` strategy.`
    chain_sync: Option<ChainSync<B, Client>>,
    /// Connected peers and their best blocks used to seed a new strategy when switching to it in
    /// `SnapcakeSyncingStrategy::proceed_to_next`.
    peer_best_blocks: HashMap<PeerId, (B::Hash, NumberFor<B>)>,
    /// Connected peers and the header of the state sync target block.
    ///
    /// Used as the target block to initiate the state sync.
    peer_state_sync_target_headers: HashMap<PeerId, B::Header>,
    /// Pending requests for best blocks.
    pending_best_block_requests: Vec<PeerId>,
    /// Pending requests for headers.
    pending_header_requests: Vec<(PeerId, BlockRequest<B>)>,
    /// Pending requests for headers by block number.
    pending_header_requests_by_number: Vec<(PeerId, NumberFor<B>)>,
    /// Pending requests for coins_count.
    pending_coins_count_requests: Vec<(PeerId, B::Hash)>,
    /// Map of block hash to the number of total coins at this block.
    coins_count: HashMap<B::Hash, u64>,
    /// Whether the state sync is finished.
    state_sync_complete: bool,
    /// Whether to skip proof in state sync.
    skip_proof: bool,
    /// Snapshot directory.
    snapshot_dir: PathBuf,
    /// Target block of the state sync.
    sync_target: TargetBlock<B>,
    /// Subcoin network request protocol name.
    subcoin_network_request_protocol_name: ProtocolName,
}

impl<B, Client> SnapcakeSyncingStrategy<B, Client>
where
    B: BlockT,
    Client: HeaderBackend<B>
        + BlockBackend<B>
        + HeaderMetadata<B, Error = sp_blockchain::Error>
        + ProofProvider<B>
        + Send
        + Sync
        + 'static,
{
    /// Initialize a new syncing strategy.
    pub fn new(
        mut config: PolkadotSyncingStrategyConfig<B>,
        client: Arc<Client>,
        skip_proof: bool,
        snapshot_dir: PathBuf,
        sync_target: TargetBlock<B>,
        subcoin_network_request_protocol_name: ProtocolName,
    ) -> Result<Self, sp_blockchain::Error> {
        if config.max_blocks_per_request > MAX_BLOCKS_IN_RESPONSE as u32 {
            tracing::info!(
                target: LOG_TARGET,
                "clamping maximum blocks per request to {MAX_BLOCKS_IN_RESPONSE}",
            );
            config.max_blocks_per_request = MAX_BLOCKS_IN_RESPONSE as u32;
        }

        let chain_sync = ChainSync::new(
            chain_sync_mode(config.mode),
            client.clone(),
            config.max_parallel_downloads,
            config.max_blocks_per_request,
            config.state_request_protocol_name.clone(),
            config.block_downloader.clone(),
            config.metrics_registry.as_ref(),
            std::iter::empty(),
        )?;

        Ok(Self {
            config,
            client,
            state: None,
            chain_sync: Some(chain_sync),
            peer_best_blocks: Default::default(),
            peer_state_sync_target_headers: HashMap::new(),
            pending_best_block_requests: Vec::new(),
            pending_header_requests: Vec::new(),
            pending_header_requests_by_number: Vec::new(),
            pending_coins_count_requests: Vec::new(),
            coins_count: HashMap::new(),
            state_sync_complete: false,
            skip_proof,
            snapshot_dir,
            sync_target,
            subcoin_network_request_protocol_name,
        })
    }

    /// Proceed with the next strategy if the active one finished.
    pub fn proceed_to_next(&mut self) -> Result<(), sp_blockchain::Error> {
        // The strategies are switched as `StateStrategy` -> `ChainSync`.
        if let Some(state) = &self.state {
            if state.is_succeeded() {
                tracing::info!(target: LOG_TARGET, "State sync is complete, continuing with block sync.");
            } else {
                tracing::error!(target: LOG_TARGET, "State sync failed. Falling back to full sync.");
            }
            self.state = None;
            Ok(())
        } else {
            unreachable!("Only warp & state strategies can finish; qed")
        }
    }

    // TODO: proper algo to select state sync target.
    fn select_state_sync_target(&self) -> Option<(&PeerId, &B::Header)> {
        self.peer_state_sync_target_headers.iter().next()
    }

    // Consume the block response and take the target block specified by the index.
    fn process_block_response(
        &mut self,
        peer_id: PeerId,
        blocks: Vec<BlockData<B>>,
        expected_target_block_index: usize,
    ) {
        // Only expect one header in the response.
        let block = blocks
            .into_iter()
            .nth(expected_target_block_index)
            .expect("Blocks must not be empty; qed");
        let target_header = block.header.expect("Header must exist as requested");
        let block_hash = target_header.hash();
        self.peer_state_sync_target_headers
            .insert(peer_id, target_header);
        self.pending_coins_count_requests
            .push((peer_id, block_hash));
    }

    fn on_block_response(
        &mut self,
        peer_id: PeerId,
        request: BlockRequest<B>,
        blocks: Vec<BlockData<B>>,
    ) {
        match self.sync_target {
            TargetBlock::Number(_) => {
                // Only expect one header in the response.
                self.process_block_response(peer_id, blocks, 0);
            }
            TargetBlock::Hash(_) | TargetBlock::LastFinalized => {
                // Process the header requests only and ignore any other block response.
                if let Some(index) = self
                    .pending_header_requests
                    .iter()
                    .position(|x| x.0 == peer_id && x.1 == request)
                {
                    tracing::debug!(target: LOG_TARGET, "============ [on_block_response] blocks: {blocks:?}");
                    self.pending_header_requests.remove(index);

                    let target_block_index = if matches!(
                        self.sync_target,
                        TargetBlock::LastFinalized
                    ) {
                        // TODO: validate_blocks
                        if blocks.len() < 6 {
                            tracing::error!(target: LOG_TARGET, "No finalized block in the block response: {blocks:?}");
                            return;
                        }
                        5
                    } else {
                        0
                    };

                    self.process_block_response(peer_id, blocks, target_block_index);
                } else {
                    // Unexpected block response.
                }
            }
        }
    }

    fn on_subcoin_response_result_v1(
        &mut self,
        peer_id: PeerId,
        response_result: Result<v1::NetworkResponse<B>, String>,
    ) {
        let response = match response_result {
            Ok(res) => res,
            Err(err) => {
                tracing::warn!(target: LOG_TARGET, "Peer {peer_id} failed to process our request: {err}");
                return;
            }
        };

        match response {
            v1::NetworkResponse::<B>::BestBlock {
                best_hash,
                best_number,
            } => {
                self.on_peer_best_block(peer_id, best_hash, best_number);
            }
            v1::NetworkResponse::<B>::CoinsCount { block_hash, count } => {
                self.coins_count.insert(block_hash, count);
            }
            v1::NetworkResponse::<B>::BlockHeader { block_header } => {
                let block_hash = block_header.hash();
                self.peer_state_sync_target_headers
                    .insert(peer_id, block_header);
                self.pending_coins_count_requests
                    .push((peer_id, block_hash));
            }
        }
    }

    fn create_subcoin_request_action(
        &self,
        peer_id: sc_network::PeerId,
        request: subcoin_service::network_request_handler::VersionedNetworkRequest<B>,
        network_service: &NetworkServiceHandle,
    ) -> SyncingAction<B> {
        let (tx, rx) = futures::channel::oneshot::channel();

        network_service.start_request(
            peer_id,
            self.subcoin_network_request_protocol_name.clone(),
            request.encode(),
            tx,
            sc_network::IfDisconnected::ImmediateError,
        );

        SyncingAction::StartRequest {
            peer_id,
            key: SUBCOIN_STRATEGY_KEY,
            request: async move {
                Ok(rx.await?.map(|(response, protocol_name)| {
                    (Box::new(response) as Box<dyn Any + Send>, protocol_name)
                }))
            }
            .boxed(),
            remove_obsolete: false,
        }
    }

    fn on_peer_best_block(
        &mut self,
        peer_id: PeerId,
        best_hash: B::Hash,
        best_number: NumberFor<B>,
    ) {
        self.peer_best_blocks
            .insert(peer_id, (best_hash, best_number));

        if let Some(s) = self.state.as_mut() {
            s.add_peer(peer_id, best_hash, best_number);
        }
        if let Some(c) = self.chain_sync.as_mut() {
            c.add_peer(peer_id, best_hash, best_number);
        }

        // Request the header of target block.
        match self.sync_target {
            TargetBlock::Number(block_number) => {
                self.pending_header_requests_by_number
                    .push((peer_id, block_number));
            }
            TargetBlock::Hash(target_block_hash) => {
                let request = BlockRequest::<B> {
                    id: 0u64,
                    fields: BlockAttributes::HEADER,
                    from: FromBlock::Hash(target_block_hash),
                    direction: Direction::Descending,
                    // Only download the specified header.
                    max: Some(1),
                };
                self.pending_header_requests.push((peer_id, request));
            }
            TargetBlock::LastFinalized => {
                let request = BlockRequest::<B> {
                    id: 0u64,
                    fields: BlockAttributes::HEADER,
                    from: FromBlock::Hash(best_hash),
                    direction: Direction::Descending,
                    // Attempt to download the most recent finalized block, i.e.,
                    // `peer_best_block - confirmation_depth(6)`.
                    max: Some(6),
                };
                self.pending_header_requests.push((peer_id, request));
            }
        }
    }
}

impl<B: BlockT, Client> SyncingStrategy<B> for SnapcakeSyncingStrategy<B, Client>
where
    B: BlockT,
    Client: HeaderBackend<B>
        + BlockBackend<B>
        + HeaderMetadata<B, Error = sp_blockchain::Error>
        + ProofProvider<B>
        + Send
        + Sync
        + 'static,
{
    fn add_peer(&mut self, peer_id: PeerId, _best_hash: B::Hash, _best_number: NumberFor<B>) {
        // NOTE: the best block info can be incorrect if the peer is still in the initial sync
        // stage, therefore we start the request best block again.
        self.pending_best_block_requests.push(peer_id);
    }

    fn remove_peer(&mut self, peer_id: &PeerId) {
        if let Some(s) = self.state.as_mut() {
            s.remove_peer(peer_id);
        }
        if let Some(c) = self.chain_sync.as_mut() {
            c.remove_peer(peer_id);
        }

        self.peer_best_blocks.remove(peer_id);
        self.pending_header_requests.retain(|(id, _)| id != peer_id);
        self.pending_header_requests_by_number
            .retain(|(id, _)| id != peer_id);
        self.pending_coins_count_requests
            .retain(|(id, _)| id != peer_id);
    }

    fn on_validated_block_announce(
        &mut self,
        is_best: bool,
        peer_id: PeerId,
        announce: &BlockAnnounce<B::Header>,
    ) -> Option<(B::Hash, NumberFor<B>)> {
        let new_best = if let Some(ref mut state) = self.state {
            state.on_validated_block_announce(is_best, peer_id, announce)
        } else if let Some(ref mut chain_sync) = self.chain_sync {
            chain_sync.on_validated_block_announce(is_best, peer_id, announce)
        } else {
            tracing::error!(target: LOG_TARGET, "No syncing strategy is active.");
            debug_assert!(false);
            Some((announce.header.hash(), *announce.header.number()))
        };

        if let Some(new_best) = new_best {
            if let Some(best) = self.peer_best_blocks.get_mut(&peer_id) {
                *best = new_best;
            } else {
                tracing::debug!(
                    target: LOG_TARGET,
                    "Cannot update `peer_best_blocks` as peer {peer_id} is not known to `Strategy` \
                     (already disconnected?)",
                );
            }
        }

        new_best
    }

    fn set_sync_fork_request(&mut self, peers: Vec<PeerId>, hash: &B::Hash, number: NumberFor<B>) {
        // Fork requests are only handled by `ChainSync`.
        if let Some(ref mut chain_sync) = self.chain_sync {
            chain_sync.set_sync_fork_request(peers.clone(), hash, number);
        }
    }

    fn request_justification(&mut self, hash: &B::Hash, number: NumberFor<B>) {
        // Justifications can only be requested via `ChainSync`.
        if let Some(ref mut chain_sync) = self.chain_sync {
            chain_sync.request_justification(hash, number);
        }
    }

    fn clear_justification_requests(&mut self) {
        // Justification requests can only be cleared by `ChainSync`.
        if let Some(ref mut chain_sync) = self.chain_sync {
            chain_sync.clear_justification_requests();
        }
    }

    fn on_justification_import(&mut self, hash: B::Hash, number: NumberFor<B>, success: bool) {
        // Only `ChainSync` is interested in justification import.
        if let Some(ref mut chain_sync) = self.chain_sync {
            chain_sync.on_justification_import(hash, number, success);
        }
    }

    fn on_generic_response(
        &mut self,
        peer_id: &PeerId,
        key: StrategyKey,
        protocol_name: ProtocolName,
        response: Box<dyn Any + Send>,
    ) {
        match key {
            StateStrategy::<B>::STRATEGY_KEY => {
                if let Some(state) = &mut self.state {
                    let Ok(response) = response.downcast::<Vec<u8>>() else {
                        tracing::warn!(target: LOG_TARGET, "Failed to downcast state response");
                        debug_assert!(false);
                        return;
                    };

                    state.on_state_response(peer_id, *response);
                } else {
                    tracing::error!(
                        target: LOG_TARGET,
                        "`on_generic_response()` called with unexpected key {key:?} \
                         or corresponding strategy is not active.",
                    );
                    debug_assert!(false);
                }
            }
            WarpSync::<B, Client>::STRATEGY_KEY => {
                unreachable!("Warp sync unsupported")
            }
            ChainSync::<B, Client>::STRATEGY_KEY => {
                if &protocol_name == self.config.block_downloader.protocol_name() {
                    let Ok(response) = response.downcast::<(
                        BlockRequest<B>,
                        Result<Vec<BlockData<B>>, BlockResponseError>,
                    )>() else {
                        tracing::warn!(target: LOG_TARGET, "Failed to downcast block response");
                        debug_assert!(false);
                        return;
                    };

                    let (request, response) = *response;
                    let blocks = match response {
                        Ok(blocks) => blocks,
                        Err(BlockResponseError::DecodeFailed(e)) => {
                            tracing::debug!(
                                target: LOG_TARGET,
                                "Failed to decode block response from peer {peer_id:?}: {e:?}.",
                            );
                            // self.actions.push(SyncingAction::DropPeer(BadPeer(*peer_id, rep::BAD_MESSAGE)));
                            return;
                        }
                        Err(BlockResponseError::ExtractionFailed(e)) => {
                            tracing::debug!(
                                target: LOG_TARGET,
                                "Failed to extract blocks from peer response {peer_id:?}: {e:?}.",
                            );
                            // self.actions.push(SyncingAction::DropPeer(BadPeer(*peer_id, rep::BAD_MESSAGE)));
                            return;
                        }
                    };

                    self.on_block_response(*peer_id, request, blocks);
                } else {
                    tracing::debug!(target: LOG_TARGET, "Ignored chain sync reponse, protocol_name: {protocol_name:?}");
                }
            }
            SUBCOIN_STRATEGY_KEY => {
                let Ok(response) = response.downcast::<Vec<u8>>() else {
                    tracing::warn!(target: LOG_TARGET, "Failed to downcast VersionedNetworkResponse");
                    debug_assert!(false);
                    return;
                };
                let response = match VersionedNetworkResponse::<B>::decode(&mut response.as_slice())
                {
                    Ok(res) => res,
                    Err(err) => {
                        tracing::warn!(target: LOG_TARGET, "Failed to decode VersionedNetworkResponse: {err:?}");
                        return;
                    }
                };

                match response {
                    VersionedNetworkResponse::V1(v1_response_result) => {
                        self.on_subcoin_response_result_v1(*peer_id, v1_response_result)
                    }
                }
            }
            key => {
                tracing::warn!(
                    target: LOG_TARGET,
                    "Unexpected generic response strategy key {key:?}, protocol {protocol_name}",
                );
                debug_assert!(false);
            }
        }
    }

    fn on_blocks_processed(
        &mut self,
        _imported: usize,
        _count: usize,
        _results: Vec<(
            Result<BlockImportStatus<NumberFor<B>>, BlockImportError>,
            B::Hash,
        )>,
    ) {
        // We are not interested in block processing notifications as we don't process any blocks.
    }

    fn on_block_finalized(&mut self, _hash: &B::Hash, _number: NumberFor<B>) {
        // We are not interested in block finalization notifications.
    }

    fn update_chain_info(&mut self, best_hash: &B::Hash, best_number: NumberFor<B>) {
        // This is relevant to `ChainSync` only.
        if let Some(ref mut chain_sync) = self.chain_sync {
            chain_sync.update_chain_info(best_hash, best_number);
        }
    }

    fn is_major_syncing(&self) -> bool {
        self.state.is_some()
            || match self.chain_sync {
                Some(ref s) => s.status().state.is_major_syncing(),
                None => unreachable!("At least one syncing strategy is active; qed"),
            }
    }

    fn num_peers(&self) -> usize {
        self.peer_best_blocks.len()
    }

    fn status(&self) -> SyncStatus<B> {
        // This function presumes that strategies are executed serially and must be refactored
        // once we have parallel strategies.
        if let Some(ref state) = self.state {
            state.status()
        } else if let Some(ref chain_sync) = self.chain_sync {
            chain_sync.status()
        } else {
            unreachable!("At least one syncing strategy is always active; qed")
        }
    }

    fn num_downloaded_blocks(&self) -> usize {
        self.chain_sync
            .as_ref()
            .map_or(0, |chain_sync| chain_sync.num_downloaded_blocks())
    }

    fn num_sync_requests(&self) -> usize {
        self.chain_sync
            .as_ref()
            .map_or(0, |chain_sync| chain_sync.num_sync_requests())
    }

    fn actions(
        &mut self,
        network_service: &NetworkServiceHandle,
    ) -> Result<Vec<SyncingAction<B>>, sp_blockchain::Error> {
        if self.state.is_none() && !self.state_sync_complete {
            if let Some((peer_id, target_header)) = self.select_state_sync_target() {
                let block_hash = target_header.hash();

                // Only start the state sync when the coins count is available.
                if let Some(total_coins) = self.coins_count.get(&block_hash) {
                    let target_header = target_header.clone();
                    tracing::info!(
                        "⏳ Starting state sync from {peer_id:?}, \
                        target block #{},{block_hash}, total coins: {total_coins}",
                        target_header.number(),
                    );
                    let state_sync = StateStrategy::new_with_provider(
                        Box::new(StateSyncWrapper::new(
                            self.client.clone(),
                            target_header,
                            self.skip_proof,
                            self.snapshot_dir.clone(),
                            *total_coins as usize,
                        )),
                        self.peer_best_blocks
                            .iter()
                            .map(|(peer_id, (_, best_number))| (*peer_id, *best_number)),
                        self.config.state_request_protocol_name.clone(),
                    );
                    self.state.replace(state_sync);
                } else {
                    tracing::debug!(
                        target: LOG_TARGET,
                        "Can not start the state sync, sync target #{},{block_hash} exists, but coins count is unavailable",
                        target_header.number(),
                    );
                }
            }
        }

        // We only handle the actions of requesting best headers and the actions from state sync.
        let actions: Vec<_> = if !self.pending_best_block_requests.is_empty() {
            std::mem::take(&mut self.pending_best_block_requests)
                .into_iter()
                .map(|peer_id| {
                    let subcoin_request =
                        VersionedNetworkRequest::V1(v1::NetworkRequest::<B>::GetBestBlock);
                    self.create_subcoin_request_action(peer_id, subcoin_request, network_service)
                })
                .collect()
        } else if !self.pending_header_requests.is_empty() {
            let mut chain_sync_actions = Vec::with_capacity(self.pending_header_requests.len());
            for (peer_id, request) in self.pending_header_requests.clone() {
                chain_sync_actions.push(
                    self.chain_sync
                        .as_mut()
                        .expect("Chain sync must be available")
                        .create_block_request_action(peer_id, request),
                );
            }
            chain_sync_actions
        } else if !self.pending_coins_count_requests.is_empty() {
            std::mem::take(&mut self.pending_coins_count_requests)
                .into_iter()
                .map(|(peer_id, block_hash)| {
                    let subcoin_request =
                        VersionedNetworkRequest::V1(v1::NetworkRequest::<B>::GetCoinsCount {
                            block_hash,
                        });
                    self.create_subcoin_request_action(peer_id, subcoin_request, network_service)
                })
                .collect()
        } else if !self.pending_header_requests_by_number.is_empty() {
            std::mem::take(&mut self.pending_header_requests_by_number)
                .into_iter()
                .map(|(peer_id, block_number)| {
                    let subcoin_request =
                        VersionedNetworkRequest::V1(v1::NetworkRequest::<B>::GetBlockHeader {
                            block_number,
                        });
                    self.create_subcoin_request_action(peer_id, subcoin_request, network_service)
                })
                .collect()
        } else if let Some(ref mut state) = self.state {
            state.actions(network_service).map(Into::into).collect()
        } else {
            return Ok(Vec::new());
        };

        let state_sync_is_complete = actions
            .iter()
            .any(|action| matches!(action, SyncingAction::ImportBlocks { .. }));

        if state_sync_is_complete {
            tracing::info!("✅ State sync is complete");
            self.state.take();
            self.state_sync_complete = true;
            // Exit the entire program directly once the state sync is complete.
            std::process::exit(0);
        }

        if actions.iter().any(SyncingAction::is_finished) {
            self.proceed_to_next()?;
        }

        Ok(actions)
    }
}
