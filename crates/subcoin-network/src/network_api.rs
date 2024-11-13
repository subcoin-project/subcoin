//! This module provides the interfaces for external interaction with Subcoin network.

#[cfg(test)]
use crate::peer_connection::Direction;
use crate::sync::PeerSync;
#[cfg(test)]
use crate::sync::SyncAction;
#[cfg(test)]
use crate::Error;
use crate::PeerId;
#[cfg(test)]
use bitcoin::p2p::message::NetworkMessage;
use bitcoin::{Transaction, Txid};
use sc_utils::mpsc::TracingUnboundedSender;
use serde::{Deserialize, Serialize};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::sync::oneshot;

/// Represents the sync status of node.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncStatus {
    /// The node is idle and not currently major syncing.
    Idle,
    /// The node is downloading blocks from peers.
    ///
    /// `target` specifies the block number the node aims to reach.
    /// `peers` is a list of peers from which the node is downloading.
    Downloading { target: u32, peers: Vec<PeerId> },
    /// The node is importing downloaded blocks into the local database.
    Importing { target: u32, peers: Vec<PeerId> },
}

/// Represents the status of network.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkStatus {
    /// The number of peers currently connected to the node.
    pub num_connected_peers: usize,
    /// The total number of bytes received from the network.
    pub total_bytes_inbound: u64,
    /// The total number of bytes sent to the network.
    pub total_bytes_outbound: u64,
    /// Current sync status of the node.
    pub sync_status: SyncStatus,
}

/// Represents the result of submitting a transaction to the network.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SendTransactionResult {
    /// Transaction was submitted successfully.
    Success(Txid),
    /// An error occurred during the transaction submission.
    Failure(String),
}

/// An incoming transaction from RPC or network.
#[derive(Debug)]
pub(crate) struct IncomingTransaction {
    pub(crate) txid: Txid,
    pub(crate) transaction: Transaction,
}

/// Represents the different messages that can be sent to the network processor.
#[derive(Debug)]
pub(crate) enum NetworkProcessorMessage {
    /// Request the current network status.
    RequestNetworkStatus(oneshot::Sender<NetworkStatus>),
    /// Request the sync peers.
    RequestSyncPeers(oneshot::Sender<Vec<PeerSync>>),
    /// Request the number of inbound connected peers.
    RequestInboundPeersCount(oneshot::Sender<usize>),
    /// Request a specific transaction by its Txid.
    RequestTransaction(Txid, oneshot::Sender<Option<Transaction>>),
    /// Submit a transaction to the transaction manager.
    SendTransaction((IncomingTransaction, oneshot::Sender<SendTransactionResult>)),
    /// Enable the block sync within the chain sync component.
    StartBlockSync,
    /// Request a local addr for the connection to given peer_id if any.
    #[cfg(test)]
    RequestLocalAddr(PeerId, oneshot::Sender<Option<PeerId>>),
    #[cfg(test)]
    ProcessNetworkMessage {
        from: PeerId,
        direction: Direction,
        payload: NetworkMessage,
        result_sender: oneshot::Sender<Result<SyncAction, Error>>,
    },
    #[cfg(test)]
    ExecuteSyncAction(SyncAction, oneshot::Sender<()>),
}

/// A handle for interacting with the network processor.
///
/// This handle allows sending messages to the network processor and provides a simple
/// way to check if the node is performing a major synchronization.
#[derive(Debug, Clone)]
pub struct NetworkHandle {
    pub(crate) processor_msg_sender: TracingUnboundedSender<NetworkProcessorMessage>,
    // A simple flag to know whether the node is doing the major sync.
    pub(crate) is_major_syncing: Arc<AtomicBool>,
}

impl NetworkHandle {
    /// Provides high-level status information about network.
    ///
    /// Returns None if the `NetworkProcessor` is no longer running.
    pub async fn status(&self) -> Option<NetworkStatus> {
        let (sender, receiver) = oneshot::channel();

        self.processor_msg_sender
            .unbounded_send(NetworkProcessorMessage::RequestNetworkStatus(sender))
            .ok();

        receiver.await.ok()
    }

    /// Returns the currently syncing peers.
    pub async fn sync_peers(&self) -> Vec<PeerSync> {
        let (sender, receiver) = oneshot::channel();

        if self
            .processor_msg_sender
            .unbounded_send(NetworkProcessorMessage::RequestSyncPeers(sender))
            .is_err()
        {
            return Vec::new();
        }

        receiver.await.unwrap_or_default()
    }

    /// Retrieves a transaction by its Txid.
    pub async fn get_transaction(&self, txid: Txid) -> Option<Transaction> {
        let (sender, receiver) = oneshot::channel();

        if self
            .processor_msg_sender
            .unbounded_send(NetworkProcessorMessage::RequestTransaction(txid, sender))
            .is_err()
        {
            return None;
        }

        receiver.await.ok().flatten()
    }

    /// Sends a transaction to the network.
    pub async fn send_transaction(&self, transaction: Transaction) -> SendTransactionResult {
        let (sender, receiver) = oneshot::channel();

        let txid = transaction.compute_txid();
        let incoming_transaction = IncomingTransaction { txid, transaction };

        if self
            .processor_msg_sender
            .unbounded_send(NetworkProcessorMessage::SendTransaction((
                incoming_transaction,
                sender,
            )))
            .is_err()
        {
            return SendTransactionResult::Failure(format!(
                "Failed to send transaction ({txid}) to net processor"
            ));
        }

        receiver
            .await
            .unwrap_or(SendTransactionResult::Failure("Internal error".to_string()))
    }

    /// Starts the block sync in chain sync component.
    pub fn start_block_sync(&self) -> bool {
        self.processor_msg_sender
            .unbounded_send(NetworkProcessorMessage::StartBlockSync)
            .is_ok()
    }

    /// Returns a flag indicating whether the node is actively performing a major sync.
    pub fn is_major_syncing(&self) -> Arc<AtomicBool> {
        self.is_major_syncing.clone()
    }

    #[cfg(test)]
    pub async fn local_addr_for(&self, peer_addr: PeerId) -> Option<PeerId> {
        let (sender, receiver) = oneshot::channel();

        self.processor_msg_sender
            .unbounded_send(NetworkProcessorMessage::RequestLocalAddr(peer_addr, sender))
            .expect("Failed to request local addr");

        receiver.await.unwrap_or_default()
    }

    #[cfg(test)]
    pub async fn process_network_message(
        &self,
        from: PeerId,
        direction: Direction,
        msg: NetworkMessage,
    ) -> Result<SyncAction, Error> {
        let (sender, receiver) = oneshot::channel();

        self.processor_msg_sender
            .unbounded_send(NetworkProcessorMessage::ProcessNetworkMessage {
                from,
                direction,
                payload: msg,
                result_sender: sender,
            })
            .expect("Failed to send outbound peer message");

        receiver.await.unwrap()
    }

    #[cfg(test)]
    pub async fn execute_sync_action(&self, sync_action: SyncAction) {
        let (sender, receiver) = oneshot::channel();

        self.processor_msg_sender
            .unbounded_send(NetworkProcessorMessage::ExecuteSyncAction(
                sync_action,
                sender,
            ))
            .expect("Failed to execute sync action");

        receiver.await.unwrap();
    }
}

/// Subcoin network service interface.
#[async_trait::async_trait]
pub trait NetworkApi: Send + Sync {
    /// Whether the network instnace is running.
    fn enabled(&self) -> bool;

    /// Provides high-level status information about network.
    ///
    /// Returns None if the `NetworkProcessor` is no longer running.
    async fn status(&self) -> Option<NetworkStatus>;

    /// Returns the currently syncing peers.
    async fn sync_peers(&self) -> Vec<PeerSync>;

    /// Retrieves a transaction by its Txid.
    async fn get_transaction(&self, txid: Txid) -> Option<Transaction>;

    /// Sends a transaction to the network.
    async fn send_transaction(&self, transaction: Transaction) -> SendTransactionResult;

    /// Starts the block sync in chain sync component.
    fn start_block_sync(&self) -> bool;

    /// Whether the node is actively performing a major sync.
    fn is_major_syncing(&self) -> bool;
}

#[async_trait::async_trait]
impl NetworkApi for NetworkHandle {
    fn enabled(&self) -> bool {
        true
    }

    async fn status(&self) -> Option<NetworkStatus> {
        Self::status(&self).await
    }

    async fn sync_peers(&self) -> Vec<PeerSync> {
        Self::sync_peers(&self).await
    }

    async fn get_transaction(&self, txid: Txid) -> Option<Transaction> {
        Self::get_transaction(&self, txid).await
    }

    async fn send_transaction(&self, transaction: Transaction) -> SendTransactionResult {
        Self::send_transaction(&self, transaction).await
    }

    fn start_block_sync(&self) -> bool {
        Self::start_block_sync(&self)
    }

    fn is_major_syncing(&self) -> bool {
        self.is_major_syncing
            .load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// Subcoin network disabled.
pub struct NoNetwork;

#[async_trait::async_trait]
impl NetworkApi for NoNetwork {
    fn enabled(&self) -> bool {
        false
    }

    async fn status(&self) -> Option<NetworkStatus> {
        None
    }

    async fn sync_peers(&self) -> Vec<PeerSync> {
        Vec::new()
    }

    async fn get_transaction(&self, _txid: Txid) -> Option<Transaction> {
        None
    }

    async fn send_transaction(&self, _transaction: Transaction) -> SendTransactionResult {
        SendTransactionResult::Failure("Network service unavailble".to_string())
    }

    fn start_block_sync(&self) -> bool {
        false
    }

    fn is_major_syncing(&self) -> bool {
        false
    }
}

/// Subcoin network is disabled, but chain is syncing from other source, e.g., importing blocks
/// from the Bitcoind database.
pub struct OfflineSync;

#[async_trait::async_trait]
impl NetworkApi for OfflineSync {
    fn enabled(&self) -> bool {
        false
    }

    async fn status(&self) -> Option<NetworkStatus> {
        None
    }

    async fn sync_peers(&self) -> Vec<PeerSync> {
        Vec::new()
    }

    async fn get_transaction(&self, _txid: Txid) -> Option<Transaction> {
        None
    }

    async fn send_transaction(&self, _transaction: Transaction) -> SendTransactionResult {
        SendTransactionResult::Failure("Network service unavailble".to_string())
    }

    fn start_block_sync(&self) -> bool {
        false
    }

    fn is_major_syncing(&self) -> bool {
        // Chain is syncing in the offline mode when using import-blocks command.
        true
    }
}
