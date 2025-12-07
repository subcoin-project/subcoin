import { useEffect, useState } from "react";
import { networkApi, systemApi } from "@subcoin/shared";
import type { NetworkStatus, SyncPeers, PeerSync } from "@subcoin/shared";
import { NetworkPageSkeleton } from "../components/Skeleton";

export function NetworkPage() {
  const [networkStatus, setNetworkStatus] = useState<NetworkStatus | null>(null);
  const [syncPeers, setSyncPeers] = useState<SyncPeers | null>(null);
  const [nodeInfo, setNodeInfo] = useState<{ name: string; version: string; chain: string } | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    async function fetchData() {
      try {
        setLoading(true);
        setError(null);

        const [status, peers, name, version, chain] = await Promise.all([
          networkApi.getStatus(),
          networkApi.getSyncPeers(),
          systemApi.name(),
          systemApi.version(),
          systemApi.chain(),
        ]);

        setNetworkStatus(status);
        setSyncPeers(peers);
        setNodeInfo({ name, version, chain });
      } catch (err) {
        setError(err instanceof Error ? err.message : "Failed to fetch network data");
      } finally {
        setLoading(false);
      }
    }

    fetchData();

    // Refresh every 10 seconds
    const interval = setInterval(fetchData, 10000);
    return () => clearInterval(interval);
  }, []);

  const formatBytes = (bytes: number) => {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
    return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GB`;
  };

  if (loading && !networkStatus) {
    return <NetworkPageSkeleton />;
  }

  if (error) {
    return (
      <div className="bg-red-900/20 border border-red-800 rounded-lg p-4">
        <h3 className="text-red-400 font-medium">Error</h3>
        <p className="text-red-300 text-sm mt-1">{error}</p>
      </div>
    );
  }

  return (
    <div className="space-y-6">
      {/* Node Info */}
      {nodeInfo && (
        <div className="bg-bitcoin-dark rounded-lg border border-gray-800 p-4">
          <h2 className="text-lg font-medium text-gray-100 mb-4">Node Information</h2>
          <dl className="grid grid-cols-1 md:grid-cols-3 gap-4">
            <div>
              <dt className="text-gray-400 text-sm">Name</dt>
              <dd className="text-gray-100 mt-1">{nodeInfo.name}</dd>
            </div>
            <div>
              <dt className="text-gray-400 text-sm">Version</dt>
              <dd className="text-gray-100 mt-1">{nodeInfo.version}</dd>
            </div>
            <div>
              <dt className="text-gray-400 text-sm">Chain</dt>
              <dd className="text-gray-100 mt-1">{nodeInfo.chain}</dd>
            </div>
          </dl>
        </div>
      )}

      {/* Network Stats */}
      {networkStatus && (
        <div className="grid grid-cols-1 md:grid-cols-4 gap-4">
          <StatCard
            title="Connected Peers"
            value={networkStatus.numConnectedPeers.toString()}
          />
          <StatCard
            title="Data Received"
            value={formatBytes(networkStatus.totalBytesInbound)}
          />
          <StatCard
            title="Data Sent"
            value={formatBytes(networkStatus.totalBytesOutbound)}
          />
          <StatCard
            title="Sync Status"
            value={getSyncStatusText(networkStatus.syncStatus)}
          />
        </div>
      )}

      {/* Sync Details */}
      {networkStatus?.syncStatus && "type" in networkStatus.syncStatus && networkStatus.syncStatus.type !== "Idle" && (
        <div className="bg-bitcoin-dark rounded-lg border border-gray-800 p-4">
          <h2 className="text-lg font-medium text-gray-100 mb-4">Sync Progress</h2>
          <div className="space-y-2">
            <div className="flex justify-between text-sm">
              <span className="text-gray-400">Status</span>
              <span className="text-gray-100">{networkStatus.syncStatus.type}</span>
            </div>
            {"target" in networkStatus.syncStatus && (
              <div className="flex justify-between text-sm">
                <span className="text-gray-400">Target Block</span>
                <span className="text-gray-100">
                  {networkStatus.syncStatus.target.toLocaleString()}
                </span>
              </div>
            )}
            {"peers" in networkStatus.syncStatus && (
              <div className="flex justify-between text-sm">
                <span className="text-gray-400">Active Peers</span>
                <span className="text-gray-100">
                  {networkStatus.syncStatus.peers.length}
                </span>
              </div>
            )}
          </div>
        </div>
      )}

      {/* Peer Summary */}
      {syncPeers && (
        <div className="bg-bitcoin-dark rounded-lg border border-gray-800 p-4">
          <h2 className="text-lg font-medium text-gray-100 mb-4">Peer Summary</h2>
          <div className="grid grid-cols-1 md:grid-cols-4 gap-4">
            <div>
              <span className="text-gray-400 text-sm">Available</span>
              <p className="text-2xl font-bold text-green-400">
                {syncPeers.peerCounts.available || 0}
              </p>
            </div>
            <div>
              <span className="text-gray-400 text-sm">Downloading</span>
              <p className="text-2xl font-bold text-blue-400">
                {syncPeers.peerCounts.downloadingNew || 0}
              </p>
            </div>
            <div>
              <span className="text-gray-400 text-sm">Deprioritized</span>
              <p className="text-2xl font-bold text-yellow-400">
                {syncPeers.peerCounts.deprioritized || 0}
              </p>
            </div>
            <div>
              <span className="text-gray-400 text-sm">Best Known Block</span>
              <p className="text-2xl font-bold text-gray-100">
                {syncPeers.bestKnownBlock?.toLocaleString() || "-"}
              </p>
            </div>
          </div>
        </div>
      )}

      {/* Peer List */}
      {syncPeers && syncPeers.peerSyncDetails.length > 0 && (
        <div className="bg-bitcoin-dark rounded-lg border border-gray-800">
          <div className="px-4 py-3 border-b border-gray-800">
            <h2 className="text-lg font-medium text-gray-100">
              Connected Peers ({syncPeers.peerSyncDetails.length})
            </h2>
          </div>
          <div className="overflow-x-auto">
            <table className="w-full">
              <thead className="bg-gray-800/50">
                <tr>
                  <th className="px-4 py-2 text-left text-xs font-medium text-gray-400 uppercase">
                    Peer ID
                  </th>
                  <th className="px-4 py-2 text-left text-xs font-medium text-gray-400 uppercase">
                    Best Block
                  </th>
                  <th className="px-4 py-2 text-left text-xs font-medium text-gray-400 uppercase">
                    Latency
                  </th>
                  <th className="px-4 py-2 text-left text-xs font-medium text-gray-400 uppercase">
                    State
                  </th>
                </tr>
              </thead>
              <tbody className="divide-y divide-gray-800">
                {syncPeers.peerSyncDetails.map((peer: PeerSync, index: number) => (
                  <tr key={index} className="hover:bg-gray-800/30">
                    <td className="px-4 py-3 font-mono text-sm text-gray-300">
                      {peer.peer_id.slice(0, 16)}...
                    </td>
                    <td className="px-4 py-3 text-gray-300">
                      {peer.best_number.toLocaleString()}
                    </td>
                    <td className="px-4 py-3 text-gray-300">
                      {peer.latency} ms
                    </td>
                    <td className="px-4 py-3">
                      <PeerStateTag state={peer.state} />
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}
    </div>
  );
}

function StatCard({ title, value }: { title: string; value: string }) {
  return (
    <div className="bg-bitcoin-dark rounded-lg border border-gray-800 p-4">
      <p className="text-gray-400 text-sm">{title}</p>
      <p className="text-2xl font-bold text-gray-100 mt-1">{value}</p>
    </div>
  );
}

function PeerStateTag({ state }: { state: PeerSync["state"] }) {
  if (!state || !("type" in state)) {
    return <span className="text-gray-500">Unknown</span>;
  }

  switch (state.type) {
    case "Available":
      return (
        <span className="bg-green-900/30 text-green-400 px-2 py-0.5 rounded text-xs">
          Available
        </span>
      );
    case "Deprioritized":
      return (
        <span className="bg-yellow-900/30 text-yellow-400 px-2 py-0.5 rounded text-xs">
          Deprioritized
        </span>
      );
    case "DownloadingNew":
      return (
        <span className="bg-blue-900/30 text-blue-400 px-2 py-0.5 rounded text-xs">
          Downloading
        </span>
      );
    default:
      return <span className="text-gray-500">Unknown</span>;
  }
}

function getSyncStatusText(status: NetworkStatus["syncStatus"] | undefined): string {
  if (!status) return "-";
  if ("type" in status) {
    switch (status.type) {
      case "Idle":
        return "Idle";
      case "Downloading":
        return "Downloading";
      case "Importing":
        return "Importing";
    }
  }
  return "Unknown";
}
