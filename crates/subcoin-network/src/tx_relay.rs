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

/// Trait for querying peer transaction relay capabilities.
///
/// This trait abstracts the peer manager operations needed for tx relay,
/// enabling testing with mock implementations.
pub trait PeerTxRelayInfo {
    /// Check if peer wants to relay transactions.
    fn peer_wants_tx_relay(&self, peer: PeerId) -> bool;
    /// Get the minimum fee rate (sat/kvB) this peer wants to receive (BIP133).
    fn get_fee_filter(&self, peer: PeerId) -> Option<u64>;
}

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
    /// Announce transaction IDs to peers.
    /// Vec of (peer_id, txids_to_announce).
    AnnounceToPeers(Vec<(PeerId, Vec<Txid>)>),
    /// Send full transactions to peers.
    /// Vec of (txid, peer_id).
    ServeTxs(Vec<(Txid, PeerId)>),
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

// Methods that don't require Client trait bounds
impl<Block, Client, Pool> TxRelay<Block, Client, Pool>
where
    Block: BlockT,
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
    ///
    /// **Note**: Caller should verify peer has relay permission before calling.
    pub fn on_inv(&mut self, inv: Vec<Inventory>, from: PeerId) -> TxAction {
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
            TxAction::None
        } else {
            tracing::debug!("Requesting {} transactions from {from}", to_request.len());
            TxAction::SendGetData(to_request, from)
        }
    }

    /// Handles periodic broadcast of pending transactions.
    ///
    /// Queries mempool for transactions pending broadcast and decides which peers
    /// to announce to based on fee filters and relay permissions.
    pub fn on_tick<P>(&mut self, peer_info: &P, connected_peers: &[PeerId]) -> TxAction
    where
        P: PeerTxRelayInfo,
    {
        self.cleanup_timeouts();

        // Query pool for transactions pending broadcast
        let pending_broadcast = self.tx_pool.pending_broadcast();

        if pending_broadcast.is_empty() {
            return TxAction::None;
        }

        // Build per-peer batches: peer -> [txids to announce]
        let mut peer_txids: HashMap<PeerId, Vec<Txid>> = HashMap::new();

        for (txid, fee_rate) in pending_broadcast {
            for &peer_id in connected_peers {
                // Skip if peer doesn't want tx relay
                if !peer_info.peer_wants_tx_relay(peer_id) {
                    continue;
                }

                // Skip if we've already announced to this peer
                if let Some(announced) = self.peer_announcements.get(&peer_id) {
                    if announced.contains(&txid) {
                        continue;
                    }
                }

                // Check fee filter (BIP133)
                if let Some(peer_min_fee) = peer_info.get_fee_filter(peer_id) {
                    if fee_rate < peer_min_fee {
                        tracing::trace!(
                            "Not relaying tx {txid} to peer {peer_id}: fee rate {fee_rate} < peer's filter {peer_min_fee}"
                        );
                        continue;
                    }
                }

                // Add to this peer's batch
                peer_txids.entry(peer_id).or_default().push(txid);

                // Track announcement
                self.peer_announcements
                    .entry(peer_id)
                    .or_insert_with(HashSet::new)
                    .insert(txid);
            }
        }

        if peer_txids.is_empty() {
            TxAction::None
        } else {
            // Return batched announcements for all peers
            TxAction::AnnounceToPeers(peer_txids.into_iter().collect())
        }
    }

    /// Handles GetData requests for transactions.
    ///
    /// Returns actions to serve the requested transactions if we have them.
    /// Only returns txids for transactions that exist in our mempool.
    pub fn on_get_data(&self, inv: Vec<Inventory>, from: PeerId) -> TxAction {
        let mut txids = Vec::new();

        for item in inv {
            if let Inventory::Transaction(txid) = item {
                // Only serve transactions we actually have
                if self.tx_pool.contains(&txid) {
                    tracing::debug!("Peer {from} requested tx {txid}");
                    txids.push((txid, from));
                } else {
                    tracing::debug!("Peer {from} requested tx {txid} but we don't have it");
                }
            }
        }

        if txids.is_empty() {
            TxAction::None
        } else {
            TxAction::ServeTxs(txids)
        }
    }

    /// Called when a transaction validation completes.
    ///
    /// Handles both accepted and rejected transactions, returning appropriate
    /// actions for peer penalty or cleanup.
    pub fn on_validated_tx(
        &mut self,
        result: TxValidationResult,
        received_from: PeerId,
    ) -> TxAction {
        match result {
            TxValidationResult::Accepted { txid, .. } => {
                // Remove from pending requests
                self.requested_txs.remove(&txid);

                // The transaction will be announced via on_tick when mempool returns
                // it from pending_broadcast()
                TxAction::None
            }
            TxValidationResult::Rejected { txid, reason } => {
                // Remove from pending requests
                self.requested_txs.remove(&txid);

                // Penalize peer if this was a hard rejection (protocol violation)
                if reason.should_penalize_peer() {
                    TxAction::RecordInvalidTx(received_from)
                } else {
                    // Soft rejection - don't penalize
                    TxAction::None
                }
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::hashes::Hash;
    use std::net::SocketAddr;
    use std::str::FromStr;

    // Test infrastructure

    struct MockTxPool {
        transactions: HashMap<Txid, Arc<Transaction>>,
        pending_broadcast: Vec<(Txid, u64)>,
        validation_responses: HashMap<Txid, TxValidationResult>,
        broadcast_marked: std::sync::Mutex<Vec<Txid>>,
    }

    impl MockTxPool {
        fn new() -> Self {
            Self {
                transactions: HashMap::new(),
                pending_broadcast: Vec::new(),
                validation_responses: HashMap::new(),
                broadcast_marked: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn add_transaction(&mut self, tx: Transaction) {
            let txid = tx.compute_txid();
            self.transactions.insert(txid, Arc::new(tx));
        }

        fn set_pending_broadcast(&mut self, txs: Vec<(Txid, u64)>) {
            self.pending_broadcast = txs;
        }

        fn get_marked_broadcast(&self) -> Vec<Txid> {
            self.broadcast_marked.lock().unwrap().clone()
        }
    }

    impl TxPool for MockTxPool {
        fn validate_transaction(&self, tx: Transaction) -> TxValidationResult {
            let txid = tx.compute_txid();
            self.validation_responses
                .get(&txid)
                .cloned()
                .unwrap_or(TxValidationResult::Accepted {
                    txid,
                    fee_rate: 1000,
                })
        }

        fn contains(&self, txid: &Txid) -> bool {
            self.transactions.contains_key(txid)
        }

        fn get(&self, txid: &Txid) -> Option<Arc<Transaction>> {
            self.transactions.get(txid).cloned()
        }

        fn pending_broadcast(&self) -> Vec<(Txid, u64)> {
            self.pending_broadcast.clone()
        }

        fn mark_broadcast(&self, txids: &[Txid]) {
            self.broadcast_marked
                .lock()
                .unwrap()
                .extend_from_slice(txids);
        }

        fn iter_txids(&self) -> Box<dyn Iterator<Item = (Txid, u64)> + Send> {
            Box::new(std::iter::empty())
        }

        fn info(&self) -> subcoin_primitives::tx_pool::TxPoolInfo {
            subcoin_primitives::tx_pool::TxPoolInfo {
                size: self.transactions.len(),
                bytes: 0,
                usage: 0,
                min_fee_rate: 0,
            }
        }
    }

    struct FakePeerManager {
        relay_permissions: HashMap<PeerId, bool>,
        fee_filters: HashMap<PeerId, u64>,
    }

    impl FakePeerManager {
        fn new() -> Self {
            Self {
                relay_permissions: HashMap::new(),
                fee_filters: HashMap::new(),
            }
        }

        fn set_relay_permission(&mut self, peer: PeerId, allowed: bool) {
            self.relay_permissions.insert(peer, allowed);
        }

        fn set_fee_filter(&mut self, peer: PeerId, min_fee: u64) {
            self.fee_filters.insert(peer, min_fee);
        }
    }

    impl PeerTxRelayInfo for FakePeerManager {
        fn peer_wants_tx_relay(&self, peer: PeerId) -> bool {
            self.relay_permissions.get(&peer).copied().unwrap_or(true)
        }

        fn get_fee_filter(&self, peer: PeerId) -> Option<u64> {
            self.fee_filters.get(&peer).copied()
        }
    }

    fn test_peer(port: u16) -> PeerId {
        SocketAddr::from_str(&format!("127.0.0.1:{port}")).unwrap()
    }

    fn test_txid(n: u8) -> Txid {
        Txid::from_byte_array([n; 32])
    }

    fn test_tx(n: u8) -> Transaction {
        Transaction {
            version: bitcoin::transaction::Version::TWO,
            lock_time: bitcoin::absolute::LockTime::ZERO,
            input: vec![],
            output: vec![bitcoin::TxOut {
                value: bitcoin::Amount::from_sat(n as u64 * 1000),
                script_pubkey: bitcoin::ScriptBuf::new(),
            }],
        }
    }

    type TestBlock = sp_runtime::generic::Block<
        sp_runtime::generic::Header<u32, sp_runtime::traits::BlakeTwo256>,
        sp_runtime::OpaqueExtrinsic,
    >;
    type TestClient = ();

    // Test cases

    #[test]
    fn test_on_inv_new_transactions() {
        let pool = Arc::new(MockTxPool::new());
        let mut relay = TxRelay::<TestBlock, TestClient, _>::new(pool);
        let peer = test_peer(8333);

        let inv = vec![
            Inventory::Transaction(test_txid(1)),
            Inventory::Transaction(test_txid(2)),
        ];

        let action = relay.on_inv(inv, peer);

        match action {
            TxAction::SendGetData(invs, p) => {
                assert_eq!(p, peer);
                assert_eq!(invs.len(), 2);
                assert!(matches!(invs[0], Inventory::Transaction(_)));
            }
            _ => panic!("Expected SendGetData"),
        }
    }

    #[test]
    fn test_on_inv_already_in_mempool() {
        let tx = test_tx(1);
        let txid = tx.compute_txid();
        let mut pool = MockTxPool::new();
        pool.add_transaction(tx);
        let pool = Arc::new(pool);
        let mut relay = TxRelay::<TestBlock, TestClient, _>::new(pool);
        let peer = test_peer(8333);

        let inv = vec![Inventory::Transaction(txid)];
        let action = relay.on_inv(inv, peer);

        assert!(matches!(action, TxAction::None));
    }

    #[test]
    fn test_on_inv_already_requested() {
        let pool = Arc::new(MockTxPool::new());
        let mut relay = TxRelay::<TestBlock, TestClient, _>::new(pool);
        let peer1 = test_peer(8333);
        let peer2 = test_peer(8334);

        // First request - should trigger SendGetData
        let inv = vec![Inventory::Transaction(test_txid(1))];
        let first_action = relay.on_inv(inv.clone(), peer1);
        assert!(matches!(first_action, TxAction::SendGetData(_, p) if p == peer1));

        // Second request from different peer - should return None (already requested)
        let action = relay.on_inv(inv, peer2);
        assert!(matches!(action, TxAction::None));
    }

    #[test]
    fn test_on_inv_max_batch_limit() {
        let pool = Arc::new(MockTxPool::new());
        let mut relay = TxRelay::<TestBlock, TestClient, _>::new(pool);
        let peer = test_peer(8333);

        // Request 150 transactions (exceeds MAX_TX_REQUEST_BATCH = 100)
        let inv: Vec<_> = (0..150)
            .map(|i| Inventory::Transaction(test_txid(i as u8)))
            .collect();

        let action = relay.on_inv(inv, peer);

        match action {
            TxAction::SendGetData(invs, _) => {
                assert_eq!(invs.len(), 100);
            }
            _ => panic!("Expected SendGetData"),
        }
    }

    #[test]
    fn test_on_inv_mixed_inventory() {
        let pool = Arc::new(MockTxPool::new());
        let mut relay = TxRelay::<TestBlock, TestClient, _>::new(pool);
        let peer = test_peer(8333);

        let inv = vec![
            Inventory::Block(bitcoin::BlockHash::all_zeros()),
            Inventory::Transaction(test_txid(1)),
            Inventory::WitnessBlock(bitcoin::BlockHash::all_zeros()),
            Inventory::Transaction(test_txid(2)),
        ];

        let action = relay.on_inv(inv, peer);

        match action {
            TxAction::SendGetData(invs, _) => {
                assert_eq!(invs.len(), 2);
                assert!(invs.iter().all(|i| matches!(i, Inventory::Transaction(_))));
            }
            _ => panic!("Expected SendGetData"),
        }
    }

    #[test]
    fn test_on_inv_duplicate_txids() {
        let pool = Arc::new(MockTxPool::new());
        let mut relay = TxRelay::<TestBlock, TestClient, _>::new(pool);
        let peer = test_peer(8333);

        let inv = vec![
            Inventory::Transaction(test_txid(1)),
            Inventory::Transaction(test_txid(1)),
            Inventory::Transaction(test_txid(1)),
        ];

        let action = relay.on_inv(inv, peer);

        match action {
            TxAction::SendGetData(invs, _) => {
                // Should only request once despite duplicates
                assert_eq!(invs.len(), 1);
            }
            _ => panic!("Expected SendGetData"),
        }
    }

    #[test]
    fn test_on_tick_empty_pending() {
        let pool = Arc::new(MockTxPool::new());
        let mut relay = TxRelay::<TestBlock, TestClient, _>::new(pool);
        let peer_mgr = FakePeerManager::new();
        let peers = vec![test_peer(8333)];

        let action = relay.on_tick(&peer_mgr, &peers);
        assert!(matches!(action, TxAction::None));
    }

    #[test]
    fn test_on_tick_announces_pending() {
        let mut pool = MockTxPool::new();
        pool.set_pending_broadcast(vec![(test_txid(1), 1000), (test_txid(2), 2000)]);
        let pool = Arc::new(pool);
        let mut relay = TxRelay::<TestBlock, TestClient, _>::new(pool);
        let peer_mgr = FakePeerManager::new();
        let peers = vec![test_peer(8333)];

        let action = relay.on_tick(&peer_mgr, &peers);

        match action {
            TxAction::AnnounceToPeers(announcements) => {
                assert_eq!(announcements.len(), 1);
                let (peer, txids) = &announcements[0];
                assert_eq!(*peer, test_peer(8333));
                assert_eq!(txids.len(), 2);
            }
            _ => panic!("Expected AnnounceToPeers"),
        }
    }

    #[test]
    fn test_on_tick_respects_relay_permission() {
        let mut pool = MockTxPool::new();
        pool.set_pending_broadcast(vec![(test_txid(1), 1000)]);
        let pool = Arc::new(pool);
        let mut relay = TxRelay::<TestBlock, TestClient, _>::new(pool);

        let mut peer_mgr = FakePeerManager::new();
        peer_mgr.set_relay_permission(test_peer(8333), false);
        let peers = vec![test_peer(8333)];

        let action = relay.on_tick(&peer_mgr, &peers);
        assert!(matches!(action, TxAction::None));
    }

    #[test]
    fn test_on_tick_respects_fee_filter() {
        let mut pool = MockTxPool::new();
        pool.set_pending_broadcast(vec![(test_txid(1), 500)]); // Low fee
        let pool = Arc::new(pool);
        let mut relay = TxRelay::<TestBlock, TestClient, _>::new(pool);

        let mut peer_mgr = FakePeerManager::new();
        peer_mgr.set_fee_filter(test_peer(8333), 1000); // Higher minimum
        let peers = vec![test_peer(8333)];

        let action = relay.on_tick(&peer_mgr, &peers);
        assert!(matches!(action, TxAction::None));
    }

    #[test]
    fn test_on_tick_skips_already_announced() {
        let mut pool = MockTxPool::new();
        pool.set_pending_broadcast(vec![(test_txid(1), 1000)]);
        let pool = Arc::new(pool);
        let mut relay = TxRelay::<TestBlock, TestClient, _>::new(pool.clone());
        let peer_mgr = FakePeerManager::new();
        let peers = vec![test_peer(8333)];

        // First tick - should announce
        let action1 = relay.on_tick(&peer_mgr, &peers);
        assert!(matches!(action1, TxAction::AnnounceToPeers(_)));

        // Second tick - should skip (already announced)
        let action2 = relay.on_tick(&peer_mgr, &peers);
        assert!(matches!(action2, TxAction::None));
    }

    #[test]
    fn test_on_tick_multi_peer_batching() {
        let mut pool = MockTxPool::new();
        pool.set_pending_broadcast(vec![(test_txid(1), 1000), (test_txid(2), 2000)]);
        let pool = Arc::new(pool);
        let mut relay = TxRelay::<TestBlock, TestClient, _>::new(pool);
        let peer_mgr = FakePeerManager::new();
        let peers = vec![test_peer(8333), test_peer(8334)];

        let action = relay.on_tick(&peer_mgr, &peers);

        match action {
            TxAction::AnnounceToPeers(announcements) => {
                assert_eq!(announcements.len(), 2);
                // Each peer should get both txids
                for (_, txids) in announcements {
                    assert_eq!(txids.len(), 2);
                }
            }
            _ => panic!("Expected AnnounceToPeers"),
        }
    }

    #[test]
    fn test_on_get_data_transaction_requests() {
        let mut pool = MockTxPool::new();
        let tx1 = test_tx(1);
        let tx2 = test_tx(2);
        let txid1 = tx1.compute_txid();
        let txid2 = tx2.compute_txid();
        pool.add_transaction(tx1);
        pool.add_transaction(tx2);
        let pool = Arc::new(pool);
        let relay = TxRelay::<TestBlock, TestClient, _>::new(pool);
        let peer = test_peer(8333);

        let inv = vec![Inventory::Transaction(txid1), Inventory::Transaction(txid2)];

        let action = relay.on_get_data(inv, peer);

        match action {
            TxAction::ServeTxs(requests) => {
                assert_eq!(requests.len(), 2);
                assert!(requests.iter().all(|(_, p)| *p == peer));
            }
            _ => panic!("Expected ServeTxs"),
        }
    }

    #[test]
    fn test_on_get_data_empty_inventory() {
        let pool = Arc::new(MockTxPool::new());
        let relay = TxRelay::<TestBlock, TestClient, _>::new(pool);
        let peer = test_peer(8333);

        let action = relay.on_get_data(vec![], peer);
        assert!(matches!(action, TxAction::None));
    }

    #[test]
    fn test_on_validated_tx_accepted() {
        let pool = Arc::new(MockTxPool::new());
        let mut relay = TxRelay::<TestBlock, TestClient, _>::new(pool);
        let peer = test_peer(8333);
        let txid = test_txid(1);

        // Request a transaction
        let inv = vec![Inventory::Transaction(txid)];
        relay.on_inv(inv, peer);
        assert_eq!(relay.requested_txs.len(), 1);

        // Validate it as accepted
        let result = TxValidationResult::Accepted {
            txid,
            fee_rate: 1000,
        };
        let action = relay.on_validated_tx(result, peer);

        // Should remove from requested and return None
        assert_eq!(relay.requested_txs.len(), 0);
        assert!(matches!(action, TxAction::None));
    }

    #[test]
    fn test_on_validated_tx_soft_rejection() {
        use subcoin_primitives::tx_pool::{RejectionReason, SoftRejection};

        let pool = Arc::new(MockTxPool::new());
        let mut relay = TxRelay::<TestBlock, TestClient, _>::new(pool);
        let peer = test_peer(8333);
        let txid = test_txid(1);

        // Request a transaction
        let inv = vec![Inventory::Transaction(txid)];
        relay.on_inv(inv, peer);
        assert_eq!(relay.requested_txs.len(), 1);

        // Validate it as soft rejected (e.g., fee too low)
        let result = TxValidationResult::Rejected {
            txid,
            reason: RejectionReason::Soft(SoftRejection::FeeTooLow {
                min_kvb: 1000,
                actual_kvb: 500,
            }),
        };
        let action = relay.on_validated_tx(result, peer);

        // Should remove from requested and return None (no penalty)
        assert_eq!(relay.requested_txs.len(), 0);
        assert!(matches!(action, TxAction::None));
    }

    #[test]
    fn test_on_validated_tx_hard_rejection() {
        use subcoin_primitives::tx_pool::{HardRejection, RejectionReason};

        let pool = Arc::new(MockTxPool::new());
        let mut relay = TxRelay::<TestBlock, TestClient, _>::new(pool);
        let peer = test_peer(8333);
        let txid = test_txid(1);

        // Request a transaction
        let inv = vec![Inventory::Transaction(txid)];
        relay.on_inv(inv, peer);
        assert_eq!(relay.requested_txs.len(), 1);

        // Validate it as hard rejected (e.g., invalid signature)
        let result = TxValidationResult::Rejected {
            txid,
            reason: RejectionReason::Hard(HardRejection::ScriptValidationFailed(
                "Invalid signature".to_string(),
            )),
        };
        let action = relay.on_validated_tx(result, peer);

        // Should remove from requested and return RecordInvalidTx
        assert_eq!(relay.requested_txs.len(), 0);
        assert!(matches!(action, TxAction::RecordInvalidTx(p) if p == peer));
    }

    #[test]
    fn test_on_peer_disconnected_cleans_up_state() {
        let pool = Arc::new(MockTxPool::new());
        let mut relay = TxRelay::<TestBlock, TestClient, _>::new(pool);
        let peer = test_peer(8333);

        // Create some state for the peer
        relay
            .peer_announcements
            .insert(peer, HashSet::from([test_txid(1)]));
        relay.requested_txs.insert(
            test_txid(2),
            RequestInfo {
                from_peer: peer,
                requested_at: Instant::now(),
            },
        );

        relay.on_peer_disconnected(peer);

        assert!(!relay.peer_announcements.contains_key(&peer));
        assert!(relay.requested_txs.is_empty());
    }

    #[test]
    fn test_cleanup_timeouts_removes_old_requests() {
        let pool = Arc::new(MockTxPool::new());
        let mut relay = TxRelay::<TestBlock, TestClient, _>::new(pool);
        let peer = test_peer(8333);

        // Add a request with old timestamp
        relay.requested_txs.insert(
            test_txid(1),
            RequestInfo {
                from_peer: peer,
                requested_at: Instant::now() - Duration::from_secs(61),
            },
        );

        // Add a recent request
        relay.requested_txs.insert(
            test_txid(2),
            RequestInfo {
                from_peer: peer,
                requested_at: Instant::now(),
            },
        );

        relay.cleanup_timeouts();

        assert!(!relay.requested_txs.contains_key(&test_txid(1)));
        assert!(relay.requested_txs.contains_key(&test_txid(2)));
    }

    // Additional edge case tests

    #[test]
    fn test_on_get_data_non_transaction_inventory() {
        let mut pool = MockTxPool::new();
        let tx1 = test_tx(1);
        let tx2 = test_tx(2);
        let txid1 = tx1.compute_txid();
        let txid2 = tx2.compute_txid();
        pool.add_transaction(tx1);
        pool.add_transaction(tx2);
        let pool = Arc::new(pool);
        let relay = TxRelay::<TestBlock, TestClient, _>::new(pool);
        let peer = test_peer(8333);

        // Mix of transaction and non-transaction inventory
        let inv = vec![
            Inventory::Block(bitcoin::BlockHash::all_zeros()),
            Inventory::Transaction(txid1),
            Inventory::WitnessBlock(bitcoin::BlockHash::all_zeros()),
            Inventory::Transaction(txid2),
        ];

        let action = relay.on_get_data(inv, peer);

        // Should only serve the transactions (and ignore blocks)
        match action {
            TxAction::ServeTxs(requests) => {
                assert_eq!(requests.len(), 2);
                assert!(
                    requests
                        .iter()
                        .all(|(txid, _)| *txid == txid1 || *txid == txid2)
                );
            }
            _ => panic!("Expected ServeTxs"),
        }
    }

    #[test]
    fn test_on_get_data_missing_transaction() {
        let mut pool = MockTxPool::new();
        // Only add tx(1) to the pool
        let tx1 = test_tx(1);
        let tx1_txid = tx1.compute_txid();
        pool.add_transaction(tx1);
        let pool = Arc::new(pool);
        let relay = TxRelay::<TestBlock, TestClient, _>::new(pool);
        let peer = test_peer(8333);

        // Request both tx(1) which exists and tx(2) which doesn't
        let inv = vec![
            Inventory::Transaction(tx1_txid),
            Inventory::Transaction(test_txid(2)), // This doesn't exist in pool
        ];

        let action = relay.on_get_data(inv, peer);

        // Should only return tx(1) which we have
        match action {
            TxAction::ServeTxs(requests) => {
                assert_eq!(requests.len(), 1);
                assert_eq!(requests[0].0, tx1_txid);
            }
            _ => panic!("Expected ServeTxs"),
        }
    }

    #[test]
    fn test_on_get_data_all_missing() {
        let pool = Arc::new(MockTxPool::new());
        let relay = TxRelay::<TestBlock, TestClient, _>::new(pool);
        let peer = test_peer(8333);

        // Request transactions that don't exist
        let inv = vec![
            Inventory::Transaction(test_txid(1)),
            Inventory::Transaction(test_txid(2)),
        ];

        let action = relay.on_get_data(inv, peer);

        // Should return None since we have none of them
        assert!(matches!(action, TxAction::None));
    }

    #[test]
    fn test_on_tick_no_peers() {
        let mut pool = MockTxPool::new();
        pool.set_pending_broadcast(vec![(test_txid(1), 1000)]);
        let pool = Arc::new(pool);
        let mut relay = TxRelay::<TestBlock, TestClient, _>::new(pool);
        let peer_mgr = FakePeerManager::new();

        // Empty peers list
        let action = relay.on_tick(&peer_mgr, &[]);
        assert!(matches!(action, TxAction::None));
    }

    #[test]
    fn test_on_tick_all_peers_already_announced() {
        let mut pool = MockTxPool::new();
        pool.set_pending_broadcast(vec![(test_txid(1), 1000)]);
        let pool = Arc::new(pool);
        let mut relay = TxRelay::<TestBlock, TestClient, _>::new(pool.clone());
        let peer_mgr = FakePeerManager::new();
        let peers = vec![test_peer(8333), test_peer(8334)];

        // First tick - should announce to all peers
        let action1 = relay.on_tick(&peer_mgr, &peers);
        assert!(matches!(action1, TxAction::AnnounceToPeers(_)));

        // Second tick - should return None (already announced to all)
        let action2 = relay.on_tick(&peer_mgr, &peers);
        assert!(matches!(action2, TxAction::None));
    }

    #[test]
    fn test_mark_broadcast_integration() {
        let mut pool = MockTxPool::new();
        let txid1 = test_txid(1);
        let txid2 = test_txid(2);
        pool.set_pending_broadcast(vec![(txid1, 1000), (txid2, 2000)]);
        let pool = Arc::new(pool);
        let mut relay = TxRelay::<TestBlock, TestClient, _>::new(pool.clone());
        let peer_mgr = FakePeerManager::new();
        let peers = vec![test_peer(8333)];

        // Verify nothing marked initially
        assert_eq!(pool.get_marked_broadcast().len(), 0);

        // Get announcement action
        let action = relay.on_tick(&peer_mgr, &peers);

        // Simulate what NetworkProcessor does: extract txids and mark as broadcast
        match action {
            TxAction::AnnounceToPeers(announcements) => {
                for (_peer, txids) in announcements {
                    // This simulates the mark_broadcast call in NetworkProcessor.do_tx_action
                    relay.mark_broadcast(&txids);
                }
            }
            _ => panic!("Expected AnnounceToPeers"),
        }

        // Verify txids were marked
        let marked = pool.get_marked_broadcast();
        assert_eq!(marked.len(), 2);
        assert!(marked.contains(&txid1));
        assert!(marked.contains(&txid2));
    }
}
