use crate::peer_manager::PeerManager;
use crate::{Error, PeerId};
use bitcoin::p2p::message_blockdata::Inventory;
use bitcoin::{Transaction, Txid};
use sp_runtime::traits::Block as BlockT;
use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::{Duration, Instant};
use subcoin_primitives::tx_pool::{TxPool, TxValidationResult};

/// Timeout for transaction requests (if we don't receive after requesting).
const TX_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Maximum number of transactions to request from a single peer at once.
const MAX_TX_REQUEST_BATCH: usize = 100;

/// Information about a requested transaction.
#[derive(Debug, Clone)]
struct RequestInfo {
    /// Peer we requested this transaction from.
    from_peer: PeerId,
    /// When we made the request.
    requested_at: Instant,
}

/// Actions to be taken by the network processor in response to transaction relay events.
#[derive(Debug)]
pub enum TxAction {
    /// No action needed.
    None,
    /// Send GetData message to request transactions.
    SendGetData(Vec<Inventory>, PeerId),
    /// Announce transaction IDs to specific peers.
    AnnounceTopeers(Txid, Vec<PeerId>),
    /// Send full transaction to a peer.
    ServeTx(Txid, PeerId),
    /// Record that a peer sent an invalid transaction.
    RecordInvalidTx(PeerId),
    /// Disconnect a peer due to protocol violation.
    DisconnectPeer(PeerId, Error),
}

/// Transaction relay coordinator.
///
/// This component orchestrates transaction relay and owns the transaction pool.
/// It provides a single point of access for all transaction operations, avoiding
/// state duplication with PeerManager or MemPool.
///
/// Design principles:
/// - Owns the tx_pool for encapsulation
/// - Queries PeerManager for peer capabilities (fee filters, relay permissions)
/// - Tracks only relay-specific state (announcements, pending requests)
/// - Returns TxAction for NetworkProcessor to execute
pub(crate) struct TxRelay<Block, Client, Pool> {
    /// The transaction pool for validation and storage.
    tx_pool: Arc<Pool>,

    /// Track which transactions we've announced to each peer.
    peer_announcements: HashMap<PeerId, HashSet<Txid>>,

    /// Track transactions we've requested but not received yet.
    requested_txs: HashMap<Txid, RequestInfo>,

    _phantom: PhantomData<(Block, Client)>,
}

impl<Block, Client, Pool> TxRelay<Block, Client, Pool>
where
    Block: BlockT,
    Client: sc_client_api::HeaderBackend<Block> + sc_client_api::AuxStore,
    Pool: TxPool,
{
    /// Creates a new transaction relay coordinator with the given pool.
    pub fn new(tx_pool: Arc<Pool>) -> Self {
        Self {
            tx_pool,
            peer_announcements: HashMap::new(),
            requested_txs: HashMap::new(),
            _phantom: PhantomData,
        }
    }

    /// Handles incoming inventory messages containing transaction announcements.
    ///
    /// Decides which transactions to request based on:
    /// - Whether we already have the transaction in mempool
    /// - Whether we've already requested it
    /// - Peer relay permissions
    pub fn on_inv(
        &mut self,
        inv: Vec<Inventory>,
        from: PeerId,
        peer_manager: &PeerManager<Block, Client>,
    ) -> Vec<TxAction> {
        // Check if peer has relay permission
        if !peer_manager.peer_wants_tx_relay(from) {
            tracing::debug!("Ignoring tx inv from {from}: no relay permission");
            return vec![TxAction::None];
        }

        let mut to_request = Vec::new();

        for item in inv {
            if let Inventory::Transaction(txid) = item {
                // Skip if we already have it in mempool
                if self.tx_pool.contains(&txid) {
                    continue;
                }

                // Skip if we've already requested it
                if self.requested_txs.contains_key(&txid) {
                    continue;
                }

                // Skip if we've recently seen it
                // TODO: Add bloom filter or cache for recently seen txids

                to_request.push(Inventory::Transaction(txid));

                // Track the request
                self.requested_txs.insert(
                    txid,
                    RequestInfo {
                        from_peer: from,
                        requested_at: Instant::now(),
                    },
                );

                // Limit batch size
                if to_request.len() >= MAX_TX_REQUEST_BATCH {
                    break;
                }
            }
        }

        if to_request.is_empty() {
            vec![TxAction::None]
        } else {
            tracing::debug!("Requesting {} transactions from {from}", to_request.len());
            vec![TxAction::SendGetData(to_request, from)]
        }
    }

    /// Handles periodic broadcast of pending transactions.
    ///
    /// Queries mempool for transactions pending broadcast and decides which peers
    /// to announce to based on fee filters and relay permissions.
    pub fn on_tick(
        &mut self,
        peer_manager: &PeerManager<Block, Client>,
        connected_peers: &[PeerId],
    ) -> Vec<TxAction> {
        self.cleanup_timeouts();

        // Query pool for transactions pending broadcast
        let unbroadcast = self.tx_pool.pending_broadcast();

        if unbroadcast.is_empty() {
            return vec![TxAction::None];
        }

        let mut actions = Vec::new();

        for (txid, fee_rate) in unbroadcast {
            let mut peers_to_announce = Vec::new();

            for &peer_id in connected_peers {
                // Skip if peer doesn't want tx relay
                if !peer_manager.peer_wants_tx_relay(peer_id) {
                    continue;
                }

                // Skip if we've already announced to this peer
                if let Some(announced) = self.peer_announcements.get(&peer_id) {
                    if announced.contains(&txid) {
                        continue;
                    }
                }

                // Check fee filter (BIP133)
                if let Some(peer_min_fee) = peer_manager.get_fee_filter(peer_id) {
                    if fee_rate < peer_min_fee {
                        tracing::trace!(
                            "Not relaying tx {txid} to peer {peer_id}: fee rate {fee_rate} < peer's filter {peer_min_fee}"
                        );
                        continue;
                    }
                }

                peers_to_announce.push(peer_id);

                // Track announcement
                self.peer_announcements
                    .entry(peer_id)
                    .or_insert_with(HashSet::new)
                    .insert(txid);
            }

            if !peers_to_announce.is_empty() {
                actions.push(TxAction::AnnounceTopeers(txid, peers_to_announce));
            }
        }

        actions
    }

    /// Handles GetData requests for transactions.
    ///
    /// Returns actions to serve the requested transactions if we have them.
    pub fn on_get_data(&self, inv: Vec<Inventory>, from: PeerId) -> Vec<TxAction> {
        let mut actions = Vec::new();

        for item in inv {
            if let Inventory::Transaction(txid) = item {
                tracing::debug!("Peer {from} requested tx {txid}");
                actions.push(TxAction::ServeTx(txid, from));
            }
        }

        actions
    }

    /// Called when a transaction has been successfully validated and added to mempool.
    ///
    /// Removes it from pending requests and prepares to relay it to other peers.
    pub fn on_validated_tx(&mut self, txid: Txid, _received_from: PeerId) -> Vec<TxAction> {
        // Remove from pending requests
        self.requested_txs.remove(&txid);

        // The transaction will be announced via on_tick when mempool returns
        // it from get_unbroadcast()
        vec![TxAction::None]
    }

    /// Called when a peer disconnects.
    ///
    /// Cleans up any per-peer state.
    pub fn on_peer_disconnected(&mut self, peer_id: PeerId) {
        self.peer_announcements.remove(&peer_id);

        // Remove pending requests from this peer
        self.requested_txs
            .retain(|_, info| info.from_peer != peer_id);
    }

    /// Removes timed-out transaction requests.
    fn cleanup_timeouts(&mut self) {
        let now = Instant::now();
        self.requested_txs.retain(|txid, info| {
            let timed_out = now.duration_since(info.requested_at) > TX_REQUEST_TIMEOUT;
            if timed_out {
                tracing::debug!("Transaction request for {txid} timed out");
            }
            !timed_out
        });
    }

    /// Validates a transaction using the internal pool.
    ///
    /// This is a forwarding method for NetworkProcessor's hot path.
    pub fn validate_transaction(&self, tx: Transaction) -> TxValidationResult {
        self.tx_pool.validate_transaction(tx)
    }

    /// Checks if a transaction is in the pool.
    pub fn contains(&self, txid: Txid) -> bool {
        self.tx_pool.contains(&txid)
    }

    /// Gets a transaction from the pool if present.
    pub fn get_transaction(&self, txid: Txid) -> Option<Arc<Transaction>> {
        self.tx_pool.get(&txid)
    }

    /// Marks transactions as broadcast to the network.
    pub fn mark_broadcast(&self, txids: &[Txid]) {
        self.tx_pool.mark_broadcast(txids);
    }
}
