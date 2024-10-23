//! This module maintains a set of high-quality peers persistently, allowing the node to connect
//! to good peers without a long discovery process.

use crate::{Latency, PeerId};
use futures::StreamExt;
use sc_utils::mpsc::{TracingUnboundedReceiver, TracingUnboundedSender};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

const PEER_STORE_FILE_NAME: &str = "peer_store.json";

/// Maximum number of persistent peers on disk.
const MAX_CAPACITY: usize = 20;

/// Periodic interval for updating `peer_store.json` on disk, in seconds.
const SAVE_INTERVAL: u64 = 60 * 5;

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
struct PeerStats {
    /// The current latency to the peer, in milliseconds.
    latency: Latency,
    /// Timestamp when the peer was last seen.
    last_seen: SystemTime,
    /// Number of blocks successfully downloaded from the peer.
    downloaded_blocks_count: usize,
    /// The number of failed interactions (e.g., disconnections, timeouts).
    failure_count: usize,
}

impl PeerStats {
    /// Compares `self` with `other` to determine which peer is better.
    ///
    /// A peer is considered better based on the following criteria (in order):
    /// 1. Fewer failures (disconnections, timeouts).
    /// 2. More blocks successfully downloaded.
    /// 3. Lower latency (better response time).
    /// 4. More recent interaction (newer `last_seen` timestamp).
    fn is_better_than(&self, other: &Self) -> bool {
        // 1. Prioritize peers with fewer failures.
        if self.failure_count != other.failure_count {
            return self.failure_count < other.failure_count;
        }

        // 2. Next, prioritize peers with more blocks downloaded.
        if self.downloaded_blocks_count != other.downloaded_blocks_count {
            return self.downloaded_blocks_count > other.downloaded_blocks_count;
        }

        // 3. Then, prioritize peers with lower latency.
        if self.latency != other.latency {
            return self.latency < other.latency;
        }

        // 4. Finally, if all else is equal, prefer peers that were seen more recently.
        self.last_seen > other.last_seen
    }
}

#[derive(Debug)]
pub(crate) enum PeerStoreMessage {
    UpdatePeer(PeerId, Latency, SystemTime),
    RemovePeer(PeerId),
    IncrementDownloadedBlocksCount(PeerId),
    IncrementFailureCount(PeerId),
}

#[derive(Clone, Debug)]
pub(crate) struct PeerStoreHandle {
    persistent_peer_latency_threshold: u128,
    sender: TracingUnboundedSender<PeerStoreMessage>,
}

impl PeerStoreHandle {
    pub(crate) fn new(
        persistent_peer_latency_threshold: u128,
        sender: TracingUnboundedSender<PeerStoreMessage>,
    ) -> Self {
        Self {
            persistent_peer_latency_threshold,
            sender,
        }
    }

    /// Adds or updates a peer in the store if its latency is below the configured threshold.
    pub(crate) fn add_peer_if_latency_acceptable(&self, peer_id: PeerId, latency: Latency) {
        if latency < self.persistent_peer_latency_threshold {
            let _ = self.sender.unbounded_send(PeerStoreMessage::UpdatePeer(
                peer_id,
                latency,
                SystemTime::now(),
            ));
        }
    }

    pub(crate) fn remove_peer(&self, peer_id: PeerId) {
        let _ = self
            .sender
            .unbounded_send(PeerStoreMessage::RemovePeer(peer_id));
    }

    pub(crate) fn increment_downloaded_blocks_count(&self, peer_id: PeerId) {
        let _ = self
            .sender
            .unbounded_send(PeerStoreMessage::IncrementDownloadedBlocksCount(peer_id));
    }

    pub(crate) fn increment_failure_count(&self, peer_id: PeerId) {
        let _ = self
            .sender
            .unbounded_send(PeerStoreMessage::IncrementFailureCount(peer_id));
    }
}

/// Manages a set of high-quality peers and periodically persists them to disk.
#[derive(Debug)]
pub(crate) struct PeerStore {
    peers: HashMap<PeerId, PeerStats>,
    sorted_peers: Vec<PeerId>,
    peers_changed: bool,
    capacity: usize,
    file_path: PathBuf,
    last_saved_at: Instant,
}

impl PeerStore {
    pub(crate) fn new(base_path: &Path, capacity: usize) -> (Self, Vec<PeerId>) {
        let file_path = base_path.join(PEER_STORE_FILE_NAME);

        let peers = load_peers(&file_path)
            .map_err(|err| {
                tracing::error!(?err, "Failed to load peers from {}", file_path.display());
            })
            .unwrap_or_default();

        let persistent_peers = peers.keys().cloned().collect::<Vec<_>>();

        let peer_store = Self {
            peers,
            sorted_peers: persistent_peers.clone(),
            capacity: capacity.min(MAX_CAPACITY),
            file_path,
            peers_changed: false,
            last_saved_at: Instant::now(),
        };

        (peer_store, persistent_peers)
    }

    pub(crate) async fn run(mut self, mut receiver: TracingUnboundedReceiver<PeerStoreMessage>) {
        while let Some(msg) = receiver.next().await {
            match msg {
                PeerStoreMessage::UpdatePeer(peer_id, latency, last_seen) => {
                    self.add_or_update_peer(peer_id, latency, last_seen);
                }
                PeerStoreMessage::RemovePeer(peer_id) => {
                    self.remove_peer(peer_id);
                }
                PeerStoreMessage::IncrementDownloadedBlocksCount(peer_id) => {
                    if let Some(peer) = self.peers.get_mut(&peer_id) {
                        peer.downloaded_blocks_count += 1;
                        self.peers_changed = true;
                        self.process_peer_changes();
                    }
                }
                PeerStoreMessage::IncrementFailureCount(peer_id) => {
                    if let Some(peer) = self.peers.get_mut(&peer_id) {
                        peer.failure_count += 1;
                        self.peers_changed = true;
                        self.process_peer_changes();
                    }
                }
            }
        }
    }

    fn add_or_update_peer(&mut self, peer_id: PeerId, latency: Latency, last_seen: SystemTime) {
        let new_peer = PeerStats {
            latency,
            last_seen,
            downloaded_blocks_count: 0,
            failure_count: 0,
        };

        if let Some(peer) = self.peers.get_mut(&peer_id) {
            peer.latency = latency;
            peer.last_seen = last_seen;
            self.peers_changed = true;
            self.process_peer_changes();
            return;
        }

        // Check if we need to replace the lowest quality peer.
        if self.peers.len() >= self.capacity {
            if let Some(lowest_peer_id) = self.sorted_peers.first() {
                let Some(lowest_peer) = self.peers.get(lowest_peer_id) else {
                    return;
                };

                if !new_peer.is_better_than(lowest_peer) {
                    // New peer is not better, no need to add.
                    return;
                }

                // Remove the worst peer if it's going to be replaced.
                self.peers.remove(lowest_peer_id);
                self.sorted_peers.remove(0);
            }
        }

        self.peers.insert(peer_id, new_peer);

        self.peers_changed = true;
        self.process_peer_changes();
    }

    /// Removes a peer from the store.
    pub fn remove_peer(&mut self, peer_id: PeerId) {
        if self.peers.remove(&peer_id).is_some() {
            self.peers_changed = true;
            self.process_peer_changes();
        }
    }

    /// Update the sorted peers and write peers to disk if needed.
    fn process_peer_changes(&mut self) {
        self.update_sorted_peers();

        // Save to disk only if peers have changed and enough time has passed since the last save.
        if self.peers_changed && self.last_saved_at.elapsed().as_secs() > SAVE_INTERVAL {
            match self.save_peers() {
                Ok(()) => {
                    self.last_saved_at = Instant::now();
                    self.peers_changed = false;
                }
                Err(err) => {
                    tracing::error!("Failed to save peers: {err:?}");
                }
            }
        }
    }

    // Sort peers based on their quality score and update sorted_peers
    fn update_sorted_peers(&mut self) {
        let mut peer_ids = self.peers.keys().cloned().collect::<Vec<_>>();
        peer_ids.sort_by(|a, b| {
            if self.peers[a].is_better_than(&self.peers[b]) {
                Ordering::Less
            } else {
                Ordering::Greater
            }
        });
        self.sorted_peers = peer_ids;
    }

    fn save_peers(&self) -> std::io::Result<()> {
        let file = std::fs::File::create(&self.file_path)?;
        serde_json::to_writer(file, &self.peers).map_err(|err| {
            std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Failed to serialize peers: {err:?}"),
            )
        })
    }
}

fn load_peers(file_path: &Path) -> std::io::Result<HashMap<PeerId, PeerStats>> {
    match std::fs::File::open(file_path) {
        Ok(mut file) => {
            let mut data = String::new();
            file.read_to_string(&mut data)?;
            Ok(serde_json::from_str(&data).map_err(|err| {
                std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Failed to deserialize peers: {err:?}"),
                )
            })?)
        }
        Err(error) => match error.kind() {
            std::io::ErrorKind::NotFound => Ok(Default::default()),
            _ => Err(error),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_peer_store() {
        let (mut peer_store, _) = PeerStore::new(&PathBuf::from("/"), 10, 200);

        let now = SystemTime::now();
        let peer1: PeerId = "127.0.0.1:8001".parse().unwrap();
        let peer2: PeerId = "127.0.0.1:8002".parse().unwrap();
        let peer3: PeerId = "127.0.0.1:8003".parse().unwrap();
        let peer4: PeerId = "127.0.0.1:8004".parse().unwrap();
        peer_store.add_or_update_peer(peer1, 10, now);
        peer_store.add_or_update_peer(peer2, 10, now.checked_sub(Duration::from_secs(1)).unwrap());
        peer_store.add_or_update_peer(peer3, 10, now.checked_add(Duration::from_secs(1)).unwrap());
        peer_store.add_or_update_peer(peer4, 5, now.checked_sub(Duration::from_secs(1)).unwrap());

        assert_eq!(peer_store.peers.len(), 4);
        assert_eq!(peer_store.sorted_peers, vec![peer4, peer3, peer1, peer2]);
    }
}
