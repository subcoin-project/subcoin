//! This module provides the interfaces for external interaction with Subcoin network.

use crate::sync::PeerSync;
use crate::PeerId;
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

/// Represents the different messages that can be sent to the network worker.
#[derive(Debug)]
pub(crate) enum NetworkWorkerMessage {
    /// Request the current network status.
    NetworkStatus(oneshot::Sender<NetworkStatus>),
    /// Request the sync peers.
    SyncPeers(oneshot::Sender<Vec<PeerSync>>),
    /// Request the number of inbound connected peers.
    InboundPeersCount(oneshot::Sender<usize>),
    /// Request a specific transaction by its Txid.
    GetTransaction((Txid, oneshot::Sender<Option<Transaction>>)),
    /// Submit a transaction to the transaction manager.
    SendTransaction((IncomingTransaction, oneshot::Sender<SendTransactionResult>)),
    /// Enable the block sync within the chain sync component.
    StartBlockSync,
}

/// A handle for interacting with the network worker.
///
/// This handle allows sending messages to the network worker and provides a simple
/// way to check if the node is performing a major synchronization.
#[derive(Debug, Clone)]
pub struct NetworkHandle {
    pub(crate) worker_msg_sender: TracingUnboundedSender<NetworkWorkerMessage>,
    // A simple flag to know whether the node is doing the major sync.
    pub(crate) is_major_syncing: Arc<AtomicBool>,
}

impl NetworkHandle {
    /// Provides high-level status information about network.
    ///
    /// Returns None if the `NetworkWorker` is no longer running.
    pub async fn status(&self) -> Option<NetworkStatus> {
        let (sender, receiver) = oneshot::channel();

        self.worker_msg_sender
            .unbounded_send(NetworkWorkerMessage::NetworkStatus(sender))
            .ok();

        receiver.await.ok()
    }

    /// Returns the currently syncing peers.
    pub async fn sync_peers(&self) -> Vec<PeerSync> {
        let (sender, receiver) = oneshot::channel();

        if self
            .worker_msg_sender
            .unbounded_send(NetworkWorkerMessage::SyncPeers(sender))
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
            .worker_msg_sender
            .unbounded_send(NetworkWorkerMessage::GetTransaction((txid, sender)))
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
            .worker_msg_sender
            .unbounded_send(NetworkWorkerMessage::SendTransaction((
                incoming_transaction,
                sender,
            )))
            .is_err()
        {
            return SendTransactionResult::Failure(format!(
                "Failed to send transaction ({txid}) to worker"
            ));
        }

        receiver
            .await
            .unwrap_or(SendTransactionResult::Failure("Internal error".to_string()))
    }

    /// Starts the block sync in chain sync component.
    pub fn start_block_sync(&self) -> bool {
        self.worker_msg_sender
            .unbounded_send(NetworkWorkerMessage::StartBlockSync)
            .is_ok()
    }

    /// Returns a flag indicating whether the node is actively performing a major sync.
    pub fn is_major_syncing(&self) -> Arc<AtomicBool> {
        self.is_major_syncing.clone()
    }
}
