use crate::metrics::Metrics;
use crate::network_api::{
    IncomingTransaction, NetworkProcessorMessage, NetworkStatus, SendTransactionResult,
};
use crate::peer_connection::{ConnectionInitiator, Direction, NewConnection};
use crate::peer_manager::{Config, NewPeer, PeerManager, SlowPeer, PEER_LATENCY_THRESHOLD};
use crate::peer_store::PeerStore;
use crate::sync::{ChainSync, LocatorRequest, RestartReason, SyncAction, SyncRequest};
use crate::transaction_manager::TransactionManager;
use crate::{Bandwidth, Error, PeerId, SyncStrategy};
use bitcoin::blockdata::block::Header as BitcoinHeader;
use bitcoin::p2p::message::{NetworkMessage, MAX_INV_SIZE};
use bitcoin::p2p::message_blockdata::{GetBlocksMessage, GetHeadersMessage, Inventory};
use bitcoin::{Block as BitcoinBlock, BlockHash};
use futures::stream::FusedStream;
use futures::StreamExt;
use sc_client_api::{AuxStore, HeaderBackend};
use sc_consensus_nakamoto::{BlockImportQueue, HeaderVerifier};
use sc_utils::mpsc::TracingUnboundedReceiver;
use sp_runtime::traits::Block as BlockT;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use subcoin_primitives::{BackendExt, ClientExt};
use substrate_prometheus_endpoint::Registry;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::MissedTickBehavior;

/// Interval at which we perform time based maintenance
const TICK_TIMEOUT: Duration = Duration::from_millis(1100);

/// Network event.
#[derive(Debug)]
pub enum Event {
    /// A new TCP stream was opened.
    NewConnection(NewConnection),
    /// Failed to make a connection to the given outbound peer.
    OutboundConnectionFailure { peer_addr: PeerId, reason: Error },
    /// TCP connection was closed, either properly or abruptly.
    DisconnectPeer { peer_addr: PeerId, reason: Error },
    /// New Bitcoin p2p network message received from the peer.
    PeerMessage {
        from: PeerId,
        direction: Direction,
        payload: NetworkMessage,
    },
}

impl Event {
    /// Constructs a [`Event::DisconnectPeer`] variant.
    pub fn disconnect(peer_addr: PeerId, reason: Error) -> Self {
        Self::DisconnectPeer { peer_addr, reason }
    }
}

/// Parameters for creating a [`NetworkProcessor`].
pub struct Params<Block, Client> {
    pub client: Arc<Client>,
    pub header_verifier: HeaderVerifier<Block, Client>,
    pub network_event_receiver: UnboundedReceiver<Event>,
    pub import_queue: BlockImportQueue,
    pub sync_strategy: SyncStrategy,
    pub is_major_syncing: Arc<AtomicBool>,
    pub connection_initiator: ConnectionInitiator,
    pub max_outbound_peers: usize,
    /// Whether to enable block sync on start.
    pub enable_block_sync: bool,
    pub peer_store: Arc<dyn PeerStore>,
}

/// [`NetworkProcessor`] is responsible for processing the network events.
pub struct NetworkProcessor<Block, Client> {
    config: Config,
    client: Arc<Client>,
    chain_sync: ChainSync<Block, Client>,
    peer_store: Arc<dyn PeerStore>,
    peer_manager: PeerManager<Block, Client>,
    header_verifier: HeaderVerifier<Block, Client>,
    transaction_manager: TransactionManager,
    network_event_receiver: UnboundedReceiver<Event>,
    /// Broadcasted blocks that are being requested.
    requested_block_announce: HashMap<PeerId, HashSet<BlockHash>>,
    metrics: Option<Metrics>,
}

impl<Block, Client> NetworkProcessor<Block, Client>
where
    Block: BlockT,
    Client: HeaderBackend<Block> + AuxStore,
{
    /// Constructs a new instance of [`NetworkProcessor`].
    pub fn new(params: Params<Block, Client>, registry: Option<&Registry>) -> Self {
        let Params {
            client,
            header_verifier,
            network_event_receiver,
            import_queue,
            sync_strategy,
            is_major_syncing,
            connection_initiator,
            max_outbound_peers,
            enable_block_sync,
            peer_store,
        } = params;

        let config = Config::new();

        let metrics = match registry {
            Some(registry) => Metrics::register(registry)
                .map_err(|err| tracing::error!("Failed to register metrics: {err}"))
                .ok(),
            None => None,
        };

        let peer_manager = PeerManager::new(
            client.clone(),
            config.clone(),
            connection_initiator,
            max_outbound_peers,
            metrics.clone(),
        );

        let chain_sync = ChainSync::new(
            client.clone(),
            header_verifier.clone(),
            import_queue,
            sync_strategy,
            is_major_syncing,
            enable_block_sync,
            peer_store.clone(),
        );

        Self {
            config,
            client,
            chain_sync,
            peer_store,
            peer_manager,
            header_verifier,
            transaction_manager: TransactionManager::new(),
            requested_block_announce: HashMap::new(),
            network_event_receiver,
            metrics,
        }
    }

    /// The main loop for processing network events.
    ///
    /// This loop handles various tasks such as processing incoming network events,
    /// syncing the blockchain, managing peers, and updating metrics. It runs indefinitely
    /// until the network processor is stopped.
    pub(crate) async fn run(
        mut self,
        processor_msg_receiver: TracingUnboundedReceiver<NetworkProcessorMessage>,
        bandwidth: Bandwidth,
    ) {
        let mut tick_timeout = {
            let mut interval = tokio::time::interval(TICK_TIMEOUT);
            interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
            interval
        };

        let mut processor_msg_sink = processor_msg_receiver.fuse();

        loop {
            tokio::select! {
                results = self.chain_sync.wait_for_block_import_results() => {
                    self.chain_sync.on_blocks_processed(results);
                }
                maybe_event = self.network_event_receiver.recv() => {
                    let Some(event) = maybe_event else {
                        return;
                    };
                    self.handle_event(event).await;
                }
                maybe_processor_msg = processor_msg_sink.next(), if !processor_msg_sink.is_terminated() => {
                    if let Some(processor_msg) = maybe_processor_msg {
                        self.handle_processor_message(processor_msg, &bandwidth).await;
                    }
                }
                _ = tick_timeout.tick() => {
                    self.execute_periodic_tasks();
                }
            }

            self.chain_sync.import_pending_blocks();
        }
    }

    async fn handle_event(&mut self, event: Event) {
        match event {
            Event::NewConnection(new_connection) => {
                self.peer_manager.on_new_connection(new_connection);
            }
            Event::OutboundConnectionFailure { peer_addr, reason } => {
                self.peer_manager
                    .on_outbound_connection_failure(peer_addr, reason);
                self.peer_store.remove_peer(peer_addr);
            }
            Event::DisconnectPeer { peer_addr, reason } => {
                self.peer_manager.disconnect(peer_addr, reason);
                self.chain_sync.disconnect(peer_addr);
                self.peer_store.remove_peer(peer_addr);
            }
            Event::PeerMessage {
                from,
                direction,
                payload,
            } => self.process_peer_message(from, direction, payload).await,
        }
    }

    async fn process_peer_message(
        &mut self,
        from: PeerId,
        direction: Direction,
        payload: NetworkMessage,
    ) {
        let msg_cmd = payload.cmd();

        tracing::trace!(?from, "Recv {msg_cmd}");

        if let Some(metrics) = &self.metrics {
            metrics
                .messages_received
                .with_label_values(&[msg_cmd])
                .inc();
        }

        match self.process_network_message(from, direction, payload).await {
            Ok(action) => self.do_sync_action(action),
            Err(err) => {
                tracing::error!(?from, ?err, "Failed to process peer message: {msg_cmd}");
            }
        }
    }

    fn execute_periodic_tasks(&mut self) {
        let sync_action = self.chain_sync.on_tick();
        self.do_sync_action(sync_action);

        for peer in self.chain_sync.unreliable_peers() {
            self.peer_manager.disconnect(peer, Error::UnreliablePeer);
            self.chain_sync.disconnect(peer);
            self.peer_store.remove_peer(peer);
        }

        let (timeout_peers, maybe_slow_peer) = self.peer_manager.on_tick();
        timeout_peers.into_iter().for_each(|peer_id| {
            self.peer_manager.disconnect(peer_id, Error::PingTimeout);
            self.chain_sync.disconnect(peer_id);
        });
        if let Some(SlowPeer { peer_id, latency }) = maybe_slow_peer {
            self.peer_manager.evict(peer_id, Error::SlowPeer(latency));
            self.chain_sync.disconnect(peer_id);
        }

        let connected_peers = self.peer_manager.connected_peers();

        for (peer, txids) in self.transaction_manager.on_tick(connected_peers) {
            tracing::debug!("Broadcasting transaction IDs {txids:?} to {peer:?}");
            let msg = NetworkMessage::Inv(txids.into_iter().map(Inventory::Transaction).collect());
            if let Err(err) = self.send(peer, msg) {
                self.peer_manager.disconnect(peer, err);
            }
        }
    }

    async fn handle_processor_message(
        &mut self,
        processor_msg: NetworkProcessorMessage,
        bandwidth: &Bandwidth,
    ) {
        match processor_msg {
            NetworkProcessorMessage::RequestNetworkStatus(result_sender) => {
                let net_status = NetworkStatus {
                    num_connected_peers: self.peer_manager.connected_peers_count(),
                    total_bytes_inbound: bandwidth.total_bytes_inbound.load(Ordering::Relaxed),
                    total_bytes_outbound: bandwidth.total_bytes_outbound.load(Ordering::Relaxed),
                    sync_status: self.chain_sync.sync_status(),
                };
                let _ = result_sender.send(net_status);
            }
            NetworkProcessorMessage::RequestSyncPeers(result_sender) => {
                let sync_peers = self.chain_sync.peers.values().cloned().collect::<Vec<_>>();
                let _ = result_sender.send(sync_peers);
            }
            NetworkProcessorMessage::RequestInboundPeersCount(result_sender) => {
                let _ = result_sender.send(self.peer_manager.inbound_peers_count());
            }
            NetworkProcessorMessage::RequestTransaction(txid, result_sender) => {
                let _ = result_sender.send(self.transaction_manager.get_transaction(&txid));
            }
            NetworkProcessorMessage::SendTransaction((incoming_transaction, result_sender)) => {
                let send_transaction_result = match self
                    .transaction_manager
                    .add_transaction(incoming_transaction)
                {
                    Ok(txid) => SendTransactionResult::Success(txid),
                    Err(error_msg) => SendTransactionResult::Failure(error_msg),
                };
                let _ = result_sender.send(send_transaction_result);
            }
            NetworkProcessorMessage::StartBlockSync => {
                let sync_action = self.chain_sync.start_block_sync();
                self.do_sync_action(sync_action);
            }
            #[cfg(test)]
            NetworkProcessorMessage::RequestLocalAddr(peer_id, result_sender) => {
                let _ = result_sender.send(self.peer_manager.local_addr(peer_id));
            }
            #[cfg(test)]
            NetworkProcessorMessage::ProcessNetworkMessage {
                from,
                direction,
                payload,
                result_sender,
            } => {
                let res = self.process_network_message(from, direction, payload).await;
                let _ = result_sender.send(res);
            }
            #[cfg(test)]
            NetworkProcessorMessage::ExecuteSyncAction(sync_action, result_sender) => {
                self.do_sync_action(sync_action);
                let _ = result_sender.send(());
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
                if let Err(err) = self.peer_manager.on_verack(from, direction) {
                    self.peer_manager.disconnect(from, err);
                }
                Ok(SyncAction::None)
            }
            NetworkMessage::Addr(addresses) => {
                self.peer_manager.on_addr(from, addresses);
                Ok(SyncAction::None)
            }
            NetworkMessage::Tx(tx) => {
                // TODO: Check has relay permission.
                let incoming_transaction = IncomingTransaction {
                    txid: tx.compute_txid(),
                    transaction: tx,
                };
                if let Err(err_msg) = self
                    .transaction_manager
                    .add_transaction(incoming_transaction)
                {
                    tracing::debug!(?from, "Failed to add transaction: {err_msg}");
                }
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
            NetworkMessage::Pong(nonce) => Ok(self.process_pong(from, nonce)?),
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
            NetworkMessage::Block(block) => self.process_block(from, block),
            NetworkMessage::Headers(headers) => self.process_headers(from, headers),
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

    fn process_pong(&mut self, from: PeerId, nonce: u64) -> Result<SyncAction, Error> {
        match self.peer_manager.on_pong(from, nonce) {
            Ok(avg_latency) => {
                // DisconnectPeer the peer directly if the latency is higher than the threshold.
                if avg_latency > PEER_LATENCY_THRESHOLD {
                    self.peer_manager
                        .disconnect(from, Error::PingLatencyTooHigh(avg_latency));
                    self.chain_sync.disconnect(from);
                    self.peer_store.remove_peer(from);
                } else {
                    self.peer_store.try_add_peer(from, avg_latency);

                    if self.chain_sync.peers.contains_key(&from) {
                        self.chain_sync.update_peer_latency(from, avg_latency);
                    } else {
                        let peer_best = self
                            .peer_manager
                            .peer_best_number(from)
                            .ok_or(Error::ConnectionNotFound(from))?;

                        let maybe_sync_start = self.chain_sync.add_new_peer(NewPeer {
                            peer_id: from,
                            best_number: peer_best,
                            latency: avg_latency,
                        });

                        return Ok(maybe_sync_start);
                    }
                }
            }
            Err(err) => {
                self.peer_manager.disconnect(from, err);
                self.chain_sync.disconnect(from);
                self.peer_store.remove_peer(from);
            }
        }

        Ok(SyncAction::None)
    }

    fn process_inv(&mut self, from: PeerId, inv: Vec<Inventory>) -> Result<SyncAction, Error> {
        if inv.len() > MAX_INV_SIZE {
            return Ok(SyncAction::DisconnectPeer(
                from,
                Error::TooManyInventoryItems,
            ));
        }

        if inv.len() == 1 {
            if let Inventory::Block(block_hash) = inv[0] {
                if self.client.block_number(block_hash).is_none() {
                    // TODO: peers may sent us duplicate block announcements.
                    tracing::debug!("Recv possible block announcement {inv:?} from {from:?}");

                    let mut is_new_block_announce = false;

                    self.requested_block_announce
                        .entry(from)
                        .and_modify(|announcements| {
                            is_new_block_announce = announcements.insert(block_hash);
                        })
                        .or_insert_with(|| {
                            is_new_block_announce = true;
                            HashSet::from([block_hash])
                        });

                    // A new block is broadcasted via `inv` message.
                    if is_new_block_announce {
                        tracing::debug!("Requesting announced block {block_hash} from {from:?}");
                        return Ok(SyncAction::get_data(inv, from));
                    }
                }
                return Ok(SyncAction::None);
            }
        }

        Ok(self.chain_sync.on_inv(inv, from))
    }

    fn process_block(&mut self, from: PeerId, block: BitcoinBlock) -> Result<SyncAction, Error> {
        let block_hash = block.block_hash();

        if self
            .requested_block_announce
            .get(&from)
            .map_or(false, |annoucements| annoucements.contains(&block_hash))
        {
            tracing::debug!("Recv announced block {block_hash} from {from:?}");

            self.requested_block_announce.entry(from).and_modify(|e| {
                e.remove(&block_hash);
            });

            if let Ok(height) = block.bip34_block_height() {
                self.chain_sync.update_peer_best(from, height as u32);
            } else {
                tracing::debug!(
                    "No height in coinbase transaction, ignored block announce {block_hash:?}"
                );
            }

            if self.client.substrate_block_hash_for(block_hash).is_some() {
                // Block has already been processed.
            } else {
                let best_hash = self
                    .client
                    .block_hash(self.client.best_number())
                    .expect("Best hash must exist; qed");

                if block.header.prev_blockhash == best_hash {
                    self.chain_sync.import_queue.import_blocks(
                        sc_consensus_nakamoto::ImportBlocks {
                            origin: sp_consensus::BlockOrigin::NetworkBroadcast,
                            blocks: vec![block],
                        },
                    );
                } else {
                    // TODO: handle the orphan block?
                    tracing::debug!("Received orphan block announce {block_hash:?}");
                }
            }

            return Ok(SyncAction::None);
        }

        Ok(self.chain_sync.on_block(block, from))
    }

    fn process_headers(
        &mut self,
        from: PeerId,
        headers: Vec<BitcoinHeader>,
    ) -> Result<SyncAction, Error> {
        if headers.is_empty() {
            return Ok(SyncAction::None);
        }

        // New blocks maybe broadcasted via `headers` message.
        let Some(best_hash) = self.client.block_hash(self.client.best_number()) else {
            return Ok(SyncAction::None);
        };

        if self.chain_sync.is_idle() {
            // New blocks, extending the chain tip.
            if headers[0].prev_blockhash == best_hash {
                for (index, header) in headers.iter().enumerate() {
                    if !self.header_verifier.has_valid_proof_of_work(header) {
                        tracing::error!(?from, "Invalid header at index {index} in headers");
                        return Ok(SyncAction::DisconnectPeer(
                            from,
                            Error::BadProofOfWork(header.block_hash()),
                        ));
                    }
                }

                let inv = headers
                    .into_iter()
                    .map(|header| {
                        let block_hash = header.block_hash();

                        self.requested_block_announce
                            .entry(from)
                            .and_modify(|e| {
                                e.insert(block_hash);
                            })
                            .or_insert_with(|| HashSet::from([block_hash]));

                        Inventory::Block(block_hash)
                    })
                    .collect();

                return Ok(SyncAction::get_data(inv, from));
            }
        }

        Ok(self.chain_sync.on_headers(headers, from))
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

    fn do_sync_action(&mut self, sync_action: SyncAction) {
        match sync_action {
            SyncAction::Request(sync_request) => match sync_request {
                SyncRequest::Header(request) => {
                    let LocatorRequest {
                        locator_hashes,
                        stop_hash,
                        to,
                    } = request;

                    if !locator_hashes.is_empty() {
                        let msg = GetHeadersMessage {
                            version: self.config.protocol_version,
                            locator_hashes,
                            stop_hash,
                        };
                        let _ = self.send(to, NetworkMessage::GetHeaders(msg));
                    }
                }
                SyncRequest::Inventory(request) => {
                    self.send_get_blocks_message(request);
                }
                SyncRequest::Data(invs, to) => {
                    if !invs.is_empty() {
                        let _ = self.send(to, NetworkMessage::GetData(invs));
                    }
                }
            },
            SyncAction::SwitchToBlocksFirstSync => {
                if let Some(SyncAction::Request(SyncRequest::Inventory(request))) =
                    self.chain_sync.attempt_blocks_first_sync()
                {
                    self.send_get_blocks_message(request);
                }
            }
            SyncAction::SetIdle => {
                self.chain_sync.set_idle();
            }
            SyncAction::RestartSyncWithStalledPeer(stalled_peer_id) => {
                self.chain_sync
                    .restart_sync(stalled_peer_id, RestartReason::Stalled);
            }
            SyncAction::DisconnectPeer(peer_id, reason) => {
                self.peer_manager.disconnect(peer_id, reason);
                self.chain_sync.disconnect(peer_id);
            }
            SyncAction::None => {}
        }
    }

    fn send_get_blocks_message(&self, request: LocatorRequest) {
        let LocatorRequest {
            locator_hashes,
            stop_hash,
            to,
        } = request;

        if !locator_hashes.is_empty() {
            let msg = GetBlocksMessage {
                version: self.config.protocol_version,
                locator_hashes,
                stop_hash,
            };
            let _ = self.send(to, NetworkMessage::GetBlocks(msg));
        }
    }

    /// Send a network message to given peer.
    #[inline]
    fn send(&self, peer_id: PeerId, network_message: NetworkMessage) -> Result<(), Error> {
        let msg_cmd = network_message.cmd();
        self.peer_manager.send(peer_id, network_message)?;
        if let Some(metrics) = &self.metrics {
            metrics.messages_sent.with_label_values(&[msg_cmd]).inc();
        }
        Ok(())
    }
}
