use sc_client_api::{BlockBackend, HeaderBackend, ProofProvider};
use sc_consensus::{BlockImportError, BlockImportStatus};
use sc_network::config::{FullNetworkConfiguration, ProtocolId};
use sc_network::service::traits::RequestResponseConfig;
use sc_network::{NetworkBackend, PeerId};
use sc_network_common::sync::message::{BlockAnnounce, BlockData, BlockRequest};
use sc_network_common::sync::SyncMode;
use sc_network_sync::state_request_handler::StateRequestHandler;
use sc_network_sync::strategy::chain_sync::{ChainSync, ChainSyncMode};
use sc_network_sync::strategy::state::StateStrategy;
use sc_network_sync::strategy::warp::EncodedProof;
use sc_network_sync::strategy::{StrategyKey, SyncingAction, SyncingConfig, SyncingStrategy};
use sc_network_sync::types::OpaqueStateResponse;
use sc_network_sync::SyncStatus;
use sc_service::SpawnTaskHandle;
use sp_blockchain::HeaderMetadata;
use sp_runtime::traits::{Block as BlockT, Header as HeaderT, NumberFor};
use std::collections::HashMap;
use std::sync::Arc;

const LOG_TARGET: &'static str = "sync";

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

/// Build Subcoin state syncing strategy
pub fn build_subcoin_syncing_strategy<Block, Client, Net>(
    protocol_id: ProtocolId,
    fork_id: Option<&str>,
    net_config: &mut FullNetworkConfiguration<Block, <Block as BlockT>::Hash, Net>,
    client: Arc<Client>,
    spawn_handle: &SpawnTaskHandle,
) -> Result<Box<dyn SyncingStrategy<Block>>, sc_service::Error>
where
    Block: BlockT,
    Client: HeaderBackend<Block>
        + BlockBackend<Block>
        + HeaderMetadata<Block, Error = sp_blockchain::Error>
        + ProofProvider<Block>
        + Send
        + Sync
        + 'static,

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

    let syncing_config = SyncingConfig {
        mode: net_config.network_config.sync_mode,
        max_parallel_downloads: net_config.network_config.max_parallel_downloads,
        max_blocks_per_request: net_config.network_config.max_blocks_per_request,
        metrics_registry: None,
        state_request_protocol_name,
    };

    Ok(Box::new(SubcoinSyncingStrategy::new(
        syncing_config,
        client,
    )?))
}

/// Proxy to specific syncing strategies used in Polkadot.
pub struct SubcoinSyncingStrategy<B: BlockT, Client> {
    /// Initial syncing configuration.
    config: SyncingConfig,
    /// Client used by syncing strategies.
    client: Arc<Client>,
    /// State strategy.
    state: Option<StateStrategy<B>>,
    /// `ChainSync` strategy.`
    chain_sync: Option<ChainSync<B, Client>>,
    /// Connected peers and their best blocks used to seed a new strategy when switching to it in
    /// `SubcoinSyncingStrategy::proceed_to_next`.
    peer_best_blocks: HashMap<PeerId, (B::Hash, NumberFor<B>)>,
}

impl<B: BlockT, Client> SyncingStrategy<B> for SubcoinSyncingStrategy<B, Client>
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
    fn add_peer(&mut self, peer_id: PeerId, best_hash: B::Hash, best_number: NumberFor<B>) {
        self.peer_best_blocks
            .insert(peer_id, (best_hash, best_number));

        self.state
            .as_mut()
            .map(|s| s.add_peer(peer_id, best_hash, best_number));
        self.chain_sync
            .as_mut()
            .map(|s| s.add_peer(peer_id, best_hash, best_number));
    }

    fn remove_peer(&mut self, peer_id: &PeerId) {
        self.state.as_mut().map(|s| s.remove_peer(peer_id));
        self.chain_sync.as_mut().map(|s| s.remove_peer(peer_id));

        self.peer_best_blocks.remove(peer_id);
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

    fn on_block_response(
        &mut self,
        peer_id: PeerId,
        key: StrategyKey,
        request: BlockRequest<B>,
        blocks: Vec<BlockData<B>>,
    ) {
        if let (StrategyKey::ChainSync, Some(ref mut chain_sync)) = (key, &mut self.chain_sync) {
            chain_sync.on_block_response(peer_id, key, request, blocks);
        } else {
            tracing::error!(
                target: LOG_TARGET,
                "`on_block_response()` called with unexpected key {key:?} \
                 or corresponding strategy is not active.",
            );
            debug_assert!(false);
        }
    }

    fn on_state_response(
        &mut self,
        peer_id: PeerId,
        key: StrategyKey,
        response: OpaqueStateResponse,
    ) {
        if let (StrategyKey::State, Some(ref mut state)) = (key, &mut self.state) {
            state.on_state_response(peer_id, response);
        } else if let (StrategyKey::ChainSync, Some(ref mut chain_sync)) =
            (key, &mut self.chain_sync)
        {
            chain_sync.on_state_response(peer_id, key, response);
        } else {
            tracing::error!(
                target: LOG_TARGET,
                "`on_state_response()` called with unexpected key {key:?} \
                 or corresponding strategy is not active.",
            );
            debug_assert!(false);
        }
    }

    fn on_warp_proof_response(
        &mut self,
        _peer_id: &PeerId,
        _key: StrategyKey,
        _response: EncodedProof,
    ) {
        unreachable!("Warp sync unsupported")
    }

    fn on_blocks_processed(
        &mut self,
        imported: usize,
        count: usize,
        results: Vec<(
            Result<BlockImportStatus<NumberFor<B>>, BlockImportError>,
            B::Hash,
        )>,
    ) {
        // Only `StateStrategy` and `ChainSync` are interested in block processing notifications.
        if let Some(ref mut state) = self.state {
            state.on_blocks_processed(imported, count, results);
        } else if let Some(ref mut chain_sync) = self.chain_sync {
            chain_sync.on_blocks_processed(imported, count, results);
        }
    }

    fn on_block_finalized(&mut self, hash: &B::Hash, number: NumberFor<B>) {
        // Only `ChainSync` is interested in block finalization notifications.
        if let Some(ref mut chain_sync) = self.chain_sync {
            chain_sync.on_block_finalized(hash, number);
        }
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

    fn actions(&mut self) -> Result<Vec<SyncingAction<B>>, sp_blockchain::Error> {
        // This function presumes that strategies are executed serially and must be refactored once
        // we have parallel strategies.
        let actions: Vec<_> = if let Some(ref mut state) = self.state {
            state.actions().map(Into::into).collect()
        } else if let Some(ref mut chain_sync) = self.chain_sync {
            chain_sync.actions()?
        } else {
            unreachable!("At least one syncing strategy is always active; qed")
        };

        if actions.iter().any(SyncingAction::is_finished) {
            self.proceed_to_next()?;
        }

        Ok(actions)
    }
}

impl<B: BlockT, Client> SubcoinSyncingStrategy<B, Client>
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
        mut config: SyncingConfig,
        client: Arc<Client>,
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
            config.metrics_registry.as_ref(),
            std::iter::empty(),
        )?;

        Ok(Self {
            config,
            client,
            state: None,
            chain_sync: Some(chain_sync),
            peer_best_blocks: Default::default(),
        })
    }

    /// Proceed with the next strategy if the active one finished.
    pub fn proceed_to_next(&mut self) -> Result<(), sp_blockchain::Error> {
        // The strategies are switched as `WarpSync` -> `StateStrategy` -> `ChainSync`.
        if let Some(state) = &self.state {
            if state.is_succeeded() {
                tracing::info!(target: LOG_TARGET, "State sync is complete, continuing with block sync.");
            } else {
                tracing::error!(target: LOG_TARGET, "State sync failed. Falling back to full sync.");
            }
            let chain_sync = match ChainSync::new(
                chain_sync_mode(self.config.mode),
                self.client.clone(),
                self.config.max_parallel_downloads,
                self.config.max_blocks_per_request,
                self.config.state_request_protocol_name.clone(),
                self.config.metrics_registry.as_ref(),
                self.peer_best_blocks
                    .iter()
                    .map(|(peer_id, (best_hash, best_number))| {
                        (*peer_id, *best_hash, *best_number)
                    }),
            ) {
                Ok(chain_sync) => chain_sync,
                Err(e) => {
                    tracing::error!(target: LOG_TARGET, "Failed to start `ChainSync`.");
                    return Err(e);
                }
            };

            self.state = None;
            self.chain_sync = Some(chain_sync);
            Ok(())
        } else {
            unreachable!("Only warp & state strategies can finish; qed")
        }
    }
}
