/**
 * Peer sync state
 */
export type PeerSyncState =
  | { type: "Available" }
  | { type: "Deprioritized" }
  | { type: "DownloadingNew"; target: number };

/**
 * Sync state category for peer counts
 */
export type SyncState = "available" | "deprioritized" | "downloadingNew";

/**
 * Peer synchronization information
 */
export interface PeerSync {
  /** Peer ID string */
  peer_id: string;
  /** Best block number known from this peer */
  best_number: number;
  /** Connection latency in milliseconds */
  latency: number;
  /** Current sync state with this peer */
  state: PeerSyncState;
}

/**
 * Overview of all sync peers
 */
export interface SyncPeers {
  /** Count of peers in each sync state */
  peerCounts: Record<SyncState, number>;
  /** Highest block known across all peers */
  bestKnownBlock: number | null;
  /** Detailed sync info per peer */
  peerSyncDetails: PeerSync[];
}

/**
 * Current sync status of the node
 */
export type SyncStatus =
  | { type: "Idle" }
  | { type: "Downloading"; target: number; peers: string[] }
  | { type: "Importing"; target: number; peers: string[] };

/**
 * Overall network status
 */
export interface NetworkStatus {
  /** Number of connected peers */
  numConnectedPeers: number;
  /** Total bytes received */
  totalBytesInbound: number;
  /** Total bytes sent */
  totalBytesOutbound: number;
  /** Current sync status */
  syncStatus: SyncStatus;
}
