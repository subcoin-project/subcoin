//! This module maintains a set of high-quality peers persistently, allowing the node to connect
//! to good peers without a long discovery process.

use crate::{Latency, PeerId};
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
struct GoodPeer {
    latency: Latency,
    last_seen: SystemTime,
}

impl GoodPeer {
    /// Returns true if `self` is a better peer than `other`.
    fn is_better_than(&self, other: &Self) -> bool {
        // First, compare latency (lower latency is better)
        if self.latency < other.latency {
            return true;
        }

        if self.latency > other.latency {
            return false;
        }

        // If latencies are the same, compare last_seen (newer is better)
        self.last_seen > other.last_seen
    }
}

/// Manages a set of high-quality peers and periodically persists them to disk.
#[derive(Debug)]
pub struct PeerStore {
    peers: HashMap<PeerId, GoodPeer>,
    sorted_peers: Vec<PeerId>,
    peers_changed: bool,
    capacity: usize,
    good_peer_latency_threshold: u128,
    file_path: PathBuf,
    last_saved_at: Instant,
}

impl PeerStore {
    pub fn new(base_path: &Path, capacity: usize, good_peer_latency_threshold: u128) -> Self {
        let file_path = base_path.join(PEER_STORE_FILE_NAME);

        let peers = load_peers(&file_path)
            .map_err(|err| {
                tracing::error!(
                    ?err,
                    "Error occurred when loading peers from {}",
                    file_path.display()
                );
            })
            .unwrap_or_default();

        let sorted_peers = peers.keys().cloned().collect();

        Self {
            peers,
            sorted_peers,
            capacity: if capacity > MAX_CAPACITY {
                MAX_CAPACITY
            } else {
                capacity
            },
            file_path,
            peers_changed: false,
            good_peer_latency_threshold,
            last_saved_at: Instant::now(),
        }
    }

    pub fn peer_set(&self) -> Vec<PeerId> {
        self.peers.keys().cloned().collect()
    }

    /// Adds or updates a peer in the store if its latency is below the configured threshold.
    pub fn add_peer_if_latency_acceptable(&mut self, peer_id: PeerId, latency: Latency) {
        if latency < self.good_peer_latency_threshold {
            self.add_or_update_peer(peer_id, latency, SystemTime::now());
        }
    }

    fn add_or_update_peer(&mut self, peer_id: PeerId, latency: Latency, last_seen: SystemTime) {
        let new_peer = GoodPeer { latency, last_seen };

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
                    self.last_saved_at = std::time::Instant::now();
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

fn load_peers(file_path: &Path) -> std::io::Result<HashMap<PeerId, GoodPeer>> {
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
        let mut peer_store = PeerStore::new(&PathBuf::from("/"), 10, 200);

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
