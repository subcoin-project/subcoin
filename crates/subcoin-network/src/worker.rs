use crate::connection::{ConnectionInitiator, Direction, NewConnection};
use crate::peer_manager::{Config, PeerManager, SlowPeer};
use crate::sync::{ChainSync, LocatorRequest, SyncAction, SyncRequest};
use crate::transaction_manager::TransactionManager;
use crate::{Bandwidth, Error, Latency, NetworkStatus, NetworkWorkerMessage, PeerId, SyncStrategy};
use bitcoin::p2p::message::{NetworkMessage, MAX_INV_SIZE};
use bitcoin::p2p::message_blockdata::{GetBlocksMessage, GetHeadersMessage, Inventory};
use futures::stream::FusedStream;
use futures::StreamExt;
use sc_client_api::{AuxStore, HeaderBackend};
use sc_consensus_nakamoto::BlockImportQueue;
use sc_utils::mpsc::TracingUnboundedReceiver;
use sp_runtime::traits::Block as BlockT;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::MissedTickBehavior;

/// Interval at which we perform time based maintenance
const TICK_TIMEOUT: Duration = Duration::from_millis(1100);

// Start with a higher threshold, e.g., 10 seconds.
const LATENCY_THRESHOLD: Latency = 10_000;

/// Network event.
#[derive(Debug)]
pub enum Event {
    /// A new TCP stream was opened.
    NewConnection(NewConnection),
    /// Failed to make a connection to the given outbound peer.
    OutboundConnectionFailure { peer_addr: PeerId, reason: Error },
    /// TCP connection was closed, either properly or abruptly.
    Disconnect { peer_addr: PeerId, reason: Error },
    /// New Bitcoin p2p network message received from the peer.
    PeerMessage {
        from: PeerId,
        direction: Direction,
        payload: NetworkMessage,
    },
}

/// Worker for processing the network events.
pub struct NetworkWorker<Block, Client> {
    config: Config,
    network_event_receiver: UnboundedReceiver<Event>,
    peer_manager: PeerManager<Block, Client>,
    transaction_manager: TransactionManager,
    chain_sync: ChainSync<Block, Client>,
}

impl<Block, Client> NetworkWorker<Block, Client>
where
    Block: BlockT,
    Client: HeaderBackend<Block> + AuxStore,
{
    /// Constructs a new instance of [`NetworkWorker`].
    pub fn new(
        client: Arc<Client>,
        network_event_receiver: UnboundedReceiver<Event>,
        import_queue: BlockImportQueue,
        sync_strategy: SyncStrategy,
        is_major_syncing: Arc<AtomicBool>,
        connection_initiator: ConnectionInitiator,
        max_outbound_peers: usize,
    ) -> Self {
        let config = Config::new();
        Self {
            network_event_receiver,
            peer_manager: PeerManager::new(
                client.clone(),
                config.clone(),
                connection_initiator,
                max_outbound_peers,
            ),
            transaction_manager: TransactionManager::new(),
            chain_sync: ChainSync::new(client, import_queue, sync_strategy, is_major_syncing),
            config,
        }
    }

    // Process the network events endlessly.
    pub(crate) async fn run(
        mut self,
        worker_msg_receiver: TracingUnboundedReceiver<NetworkWorkerMessage>,
        bandwidth: Bandwidth,
    ) {
        let mut tick_timeout = {
            let mut interval = tokio::time::interval(TICK_TIMEOUT);
            interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
            interval
        };

        let mut worker_msg_sink = worker_msg_receiver.fuse();

        loop {
            tokio::select! {
                results = self.chain_sync.wait_for_block_import_results() => {
                    self.chain_sync.on_blocks_processed(results);
                }
                maybe_event = self.network_event_receiver.recv() => {
                    let Some(event) = maybe_event else {
                        return;
                    };
                    self.process_event(event).await;
                }
                _ = tick_timeout.tick() => {
                    self.perform_periodic_actions();
                }
                maybe_worker_msg = worker_msg_sink.next(), if !worker_msg_sink.is_terminated() => {
                    if let Some(worker_msg) = maybe_worker_msg {
                        self.process_worker_message(worker_msg, &bandwidth);
                    }
                }
            }

            self.chain_sync.import_pending_blocks();
        }
    }

    async fn process_event(&mut self, event: Event) {
        match event {
            Event::NewConnection(new_connection) => {
                self.peer_manager.on_new_connection(new_connection);
            }
            Event::OutboundConnectionFailure { peer_addr, reason } => {
                self.peer_manager
                    .on_outbound_connection_failure(peer_addr, reason);
            }
            Event::Disconnect { peer_addr, reason } => {
                self.peer_manager.disconnect(peer_addr, reason);
                self.chain_sync.remove_peer(peer_addr);
            }
            Event::PeerMessage {
                from,
                direction,
                payload,
            } => {
                let msg_cmd = payload.cmd();

                tracing::trace!(?from, "Recv {msg_cmd}");

                match self.process_network_message(from, direction, payload).await {
                    Ok(action) => self.do_sync_action(action),
                    Err(err) => {
                        tracing::error!(?from, ?err, "Failed to process peer message: {msg_cmd}");
                    }
                }
            }
        }
    }

    fn perform_periodic_actions(&mut self) {
        let sync_action = self.chain_sync.on_tick();
        self.do_sync_action(sync_action);

        if let Some(SlowPeer {
            peer_id,
            peer_latency,
        }) = self.peer_manager.on_tick()
        {
            self.peer_manager
                .disconnect(peer_id, Error::SlowPeer(peer_latency));
            self.peer_manager.update_last_eviction();
            self.chain_sync.remove_peer(peer_id);
        }

        for (peer, txids) in self
            .transaction_manager
            .on_tick(self.peer_manager.connected_peers())
        {
            tracing::debug!("Broadcasting transaction IDs {txids:?} to {peer:?}");
            let msg = NetworkMessage::Inv(txids.into_iter().map(Inventory::Transaction).collect());
            if let Err(err) = self.send(peer, msg) {
                self.peer_manager.disconnect(peer, err);
            }
        }
    }

    fn process_worker_message(&mut self, worker_msg: NetworkWorkerMessage, bandwidth: &Bandwidth) {
        match worker_msg {
            NetworkWorkerMessage::NetworkStatus(result_sender) => {
                let net_status = NetworkStatus {
                    num_connected_peers: self.peer_manager.connected_peers_count(),
                    total_bytes_inbound: bandwidth.total_bytes_inbound.load(Ordering::Relaxed),
                    total_bytes_outbound: bandwidth.total_bytes_outbound.load(Ordering::Relaxed),
                    sync_status: self.chain_sync.sync_status(),
                };
                let _ = result_sender.send(net_status);
            }
            NetworkWorkerMessage::SyncPeers(result_sender) => {
                let sync_peers = self.chain_sync.peers.values().cloned().collect::<Vec<_>>();
                let _ = result_sender.send(sync_peers);
            }
            NetworkWorkerMessage::InboundPeersCount(result_sender) => {
                let _ = result_sender.send(self.peer_manager.inbound_peers_count());
            }
            NetworkWorkerMessage::GetTransaction((txid, result_sender)) => {
                let _ = result_sender.send(self.transaction_manager.get_transaction(&txid));
            }
            NetworkWorkerMessage::SendTransaction(tx) => {
                self.transaction_manager.add_transaction(tx);
            }
        }
    }

    // Ref https://github.com/bitcoin/bitcoin/blob/ac19235818e220108cf44932194af12ef6e1be8b/src/net_processing.cpp#L3382
    async fn process_network_message(
        &mut self,
        from: PeerId,
        direction: Direction,
        message: NetworkMessage,
    ) -> Result<SyncAction, Error> {
        match message {
            NetworkMessage::Version(version_message) => {
                if let Err(err) = self
                    .peer_manager
                    .on_version(from, direction, version_message)
                {
                    self.peer_manager.disconnect(from, err);
                }
                Ok(SyncAction::None)
            }
            NetworkMessage::Verack => {
                if self.peer_manager.is_connected(from) {
                    tracing::debug!(?from, "Ignoring redundant verack");
                    return Ok(SyncAction::None);
                }

                let new_peer = match self.peer_manager.on_verack(from, direction) {
                    Ok(new_peer) => new_peer,
                    Err(err) => {
                        self.peer_manager.disconnect(from, err);
                        return Ok(SyncAction::None);
                    }
                };

                Ok(self.chain_sync.add_new_peer(new_peer))
            }
            NetworkMessage::Addr(addresses) => {
                self.peer_manager.on_addr(from, addresses);
                Ok(SyncAction::None)
            }
            NetworkMessage::Tx(tx) => {
                // TODO: check if the peer is allowed to send tx.
                self.transaction_manager.add_transaction(tx);
                Ok(SyncAction::None)
            }
            NetworkMessage::GetData(inv) => {
                self.process_get_data(from, inv);
                Ok(SyncAction::None)
            }
            NetworkMessage::GetBlocks(_) => {
                self.send(from, NetworkMessage::Inv(Vec::new()))?;
                Ok(SyncAction::None)
            }
            NetworkMessage::GetHeaders(_) => {
                self.send(from, NetworkMessage::Inv(Vec::new()))?;
                Ok(SyncAction::None)
            }
            NetworkMessage::GetAddr => {
                self.send(from, NetworkMessage::AddrV2(Vec::new()))?;
                Ok(SyncAction::None)
            }
            NetworkMessage::Ping(nonce) => {
                self.send(from, NetworkMessage::Pong(nonce))?;
                Ok(SyncAction::None)
            }
            NetworkMessage::Pong(nonce) => {
                match self.peer_manager.on_pong(from, nonce) {
                    Ok(latency) => {
                        // Disconnect the peer directly if the latency is higher than the threshold.
                        if latency > LATENCY_THRESHOLD {
                            self.peer_manager
                                .disconnect(from, Error::PingLatencyTooHigh);
                            self.chain_sync.remove_peer(from);
                        } else {
                            self.chain_sync.set_peer_latency(from, latency);
                            self.chain_sync.update_sync_peer_on_lower_latency();
                        }
                    }
                    Err(err) => {
                        self.peer_manager.disconnect(from, err);
                        self.chain_sync.remove_peer(from);
                    }
                }
                Ok(SyncAction::None)
            }
            NetworkMessage::AddrV2(addresses) => {
                self.peer_manager.on_addr_v2(from, addresses);
                Ok(SyncAction::None)
            }
            NetworkMessage::SendAddrV2 => {
                self.peer_manager.set_want_addrv2(from);
                Ok(SyncAction::None)
            }
            NetworkMessage::SendHeaders => {
                self.peer_manager.set_prefer_headers(from);
                Ok(SyncAction::None)
            }
            NetworkMessage::FeeFilter(_) => {
                self.send(from, NetworkMessage::FeeFilter(1000))?;
                Ok(SyncAction::None)
            }
            NetworkMessage::Inv(inv) => self.process_inv(from, inv),
            NetworkMessage::Block(block) => Ok(self.chain_sync.on_block(block, from)),
            NetworkMessage::Headers(headers) => Ok(self.chain_sync.on_headers(headers, from)),
            NetworkMessage::MerkleBlock(_) => Ok(SyncAction::None),
            NetworkMessage::Unknown { .. }
            | NetworkMessage::NotFound(_)
            | NetworkMessage::MemPool
            | NetworkMessage::FilterLoad(_)
            | NetworkMessage::FilterAdd(_)
            | NetworkMessage::FilterClear
            | NetworkMessage::GetCFilters(_)
            | NetworkMessage::CFilter(_)
            | NetworkMessage::GetCFHeaders(_)
            | NetworkMessage::CFHeaders(_)
            | NetworkMessage::GetCFCheckpt(_)
            | NetworkMessage::CFCheckpt(_)
            | NetworkMessage::SendCmpct(_)
            | NetworkMessage::CmpctBlock(_)
            | NetworkMessage::GetBlockTxn(_)
            | NetworkMessage::BlockTxn(_)
            | NetworkMessage::Alert(_)
            | NetworkMessage::Reject(_)
            | NetworkMessage::WtxidRelay => Ok(SyncAction::None),
        }
    }

    fn do_sync_action(&mut self, sync_action: SyncAction) {
        match sync_action {
            SyncAction::Request(sync_request) => match sync_request {
                SyncRequest::Headers(request) => {
                    let LocatorRequest {
                        locator_hashes,
                        stop_hash,
                        from,
                    } = request;

                    if !locator_hashes.is_empty() {
                        let msg = GetHeadersMessage {
                            version: self.config.protocol_version,
                            locator_hashes,
                            stop_hash,
                        };
                        let _ = self.send(from, NetworkMessage::GetHeaders(msg));
                    }
                }
                SyncRequest::Blocks(request) => {
                    self.send_get_blocks_request(request);
                }
                SyncRequest::Data(invs, from) => {
                    if !invs.is_empty() {
                        let _ = self.send(from, NetworkMessage::GetData(invs));
                    }
                }
            },
            SyncAction::SwitchToBlocksFirstSync => {
                if let Some(SyncRequest::Blocks(request)) =
                    self.chain_sync.attempt_blocks_first_sync()
                {
                    self.send_get_blocks_request(request);
                }
            }
            SyncAction::RestartSyncWithStalledPeer(stalled_peer_id) => {
                if self.chain_sync.restart_sync(stalled_peer_id) {
                    self.chain_sync.mark_peer_as_discouraged(stalled_peer_id);
                }
            }
            SyncAction::Disconnect(peer_id, reason) => {
                self.peer_manager.disconnect(peer_id, reason);
                self.chain_sync.remove_peer(peer_id);
            }
            SyncAction::None => {}
        }
    }

    fn send_get_blocks_request(&self, request: LocatorRequest) {
        let LocatorRequest {
            locator_hashes,
            stop_hash,
            from,
        } = request;

        if !locator_hashes.is_empty() {
            let msg = GetBlocksMessage {
                version: self.config.protocol_version,
                locator_hashes,
                stop_hash,
            };
            let _ = self.send(from, NetworkMessage::GetBlocks(msg));
        }
    }

    fn process_inv(&mut self, from: PeerId, inv: Vec<Inventory>) -> Result<SyncAction, Error> {
        if inv.len() > MAX_INV_SIZE {
            return Ok(SyncAction::Disconnect(from, Error::TooManyInventoryItems));
        }

        Ok(self.chain_sync.on_inv(inv, from))
    }

    fn process_get_data(&self, from: PeerId, get_data_requests: Vec<Inventory>) {
        // TODO: process tx as many as possible.
        for inv in get_data_requests {
            match inv {
                Inventory::Block(_) | Inventory::CompactBlock(_) | Inventory::WitnessBlock(_) => {
                    // TODO: process one BLOCK item per call, as Bitcore Core does.
                    self.process_get_block_data(&inv);
                }
                Inventory::Transaction(txid) => {
                    tracing::debug!("Recv transaction request: {txid:?} from {from:?}");
                    if let Some(transaction) = self.transaction_manager.get_transaction(&txid) {
                        if let Err(err) = self.send(from, NetworkMessage::Tx(transaction)) {
                            tracing::error!(?err, "Failed to send transaction {txid} to {from:?}");
                        }
                    }
                }
                Inventory::WTx(_)
                | Inventory::WitnessTransaction(_)
                | Inventory::Unknown { .. }
                | Inventory::Error => {}
            }
        }
    }

    fn process_get_block_data(&self, _inv: &Inventory) {
        // TODO: load the requested block and send them back.
    }

    /// Send a network message to given peer.
    #[inline]
    fn send(&self, peer_id: PeerId, network_message: NetworkMessage) -> Result<(), Error> {
        self.peer_manager.send(peer_id, network_message)
    }
}
