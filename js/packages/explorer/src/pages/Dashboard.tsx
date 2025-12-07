import { useEffect, useState, useCallback } from "react";
import { Link } from "react-router-dom";
import { blockchainApi, networkApi, systemApi } from "@subcoin/shared";
import type { NetworkStatus } from "@subcoin/shared";
import { useNewBlockSubscription, useConnection } from "../contexts/ConnectionContext";

interface BlockInfo {
  height: number;
  hash: string;
  timestamp: number;
  txCount: number;
}

export function Dashboard() {
  const [latestBlocks, setLatestBlocks] = useState<BlockInfo[]>([]);
  const [networkStatus, setNetworkStatus] = useState<NetworkStatus | null>(null);
  const [syncState, setSyncState] = useState<{ current: number; highest?: number } | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [lastUpdate, setLastUpdate] = useState<Date | null>(null);
  const { isConnected } = useConnection();

  const fetchData = useCallback(async () => {
    try {
      setLoading(true);
      setError(null);

      // Fetch network status
      const status = await networkApi.getStatus();
      setNetworkStatus(status);

      // Fetch sync state
      const sync = await systemApi.syncState();
      setSyncState({ current: sync.currentBlock, highest: sync.highestBlock });

      // Fetch latest blocks
      const blocks: BlockInfo[] = [];
      const currentHeight = sync.currentBlock;

      // Fetch last 10 blocks
      for (let i = 0; i < 10 && currentHeight - i >= 0; i++) {
        const height = currentHeight - i;
        const block = await blockchainApi.getBlockByNumber(height);
        if (block) {
          const hash = await blockchainApi.getBlockHash(height);
          blocks.push({
            height,
            hash: hash || "",
            timestamp: block.header.time,
            txCount: block.txdata.length,
          });
        }
      }

      setLatestBlocks(blocks);
      setLastUpdate(new Date());
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to fetch data");
    } finally {
      setLoading(false);
    }
  }, []);

  // Initial fetch
  useEffect(() => {
    fetchData();
  }, [fetchData]);

  // Subscribe to new blocks via WebSocket
  useNewBlockSubscription(
    useCallback(
      (blockNumber: number) => {
        // Refresh data when a new block arrives
        fetchData();
      },
      [fetchData]
    )
  );

  // Fallback: poll every 60 seconds if WebSocket is not connected
  useEffect(() => {
    if (!isConnected) {
      const interval = setInterval(fetchData, 60000);
      return () => clearInterval(interval);
    }
  }, [isConnected, fetchData]);

  const formatTimestamp = (timestamp: number) => {
    return new Date(timestamp * 1000).toLocaleString();
  };

  const formatHash = (hash: string) => {
    if (!hash) return "";
    return `${hash.slice(0, 8)}...${hash.slice(-8)}`;
  };

  if (loading && latestBlocks.length === 0) {
    return (
      <div className="flex items-center justify-center h-64">
        <div className="text-gray-400">Loading...</div>
      </div>
    );
  }

  if (error) {
    return (
      <div className="bg-red-900/20 border border-red-800 rounded-lg p-4">
        <h3 className="text-red-400 font-medium">Error</h3>
        <p className="text-red-300 text-sm mt-1">{error}</p>
        <p className="text-gray-400 text-xs mt-2">
          Make sure a Subcoin node is running and accessible at the configured endpoint.
        </p>
      </div>
    );
  }

  const formatLastUpdate = () => {
    if (!lastUpdate) return "";
    return lastUpdate.toLocaleTimeString();
  };

  return (
    <div className="space-y-6">
      {/* Stats Cards */}
      <div className="grid grid-cols-1 md:grid-cols-4 gap-4">
        <StatCard
          title="Block Height"
          value={syncState?.current?.toLocaleString() ?? "-"}
        />
        <StatCard
          title="Connected Peers"
          value={networkStatus?.numConnectedPeers?.toString() ?? "-"}
        />
        <StatCard
          title="Sync Target"
          value={syncState?.highest?.toLocaleString() ?? "Synced"}
        />
        <StatCard
          title="Network"
          value={getSyncStatusText(networkStatus?.syncStatus)}
        />
      </div>

      {/* Latest Blocks */}
      <div className="bg-bitcoin-dark rounded-lg border border-gray-800">
        <div className="px-4 py-3 border-b border-gray-800 flex items-center justify-between">
          <h2 className="text-lg font-medium text-gray-100">Latest Blocks</h2>
          <div className="flex items-center space-x-2">
            {isConnected && (
              <span className="text-xs text-green-500 flex items-center">
                <span className="w-1.5 h-1.5 bg-green-500 rounded-full mr-1 animate-pulse" />
                Live
              </span>
            )}
            {lastUpdate && (
              <span className="text-xs text-gray-500">
                Updated {formatLastUpdate()}
              </span>
            )}
          </div>
        </div>
        <div className="overflow-x-auto">
          <table className="w-full">
            <thead className="bg-gray-800/50">
              <tr>
                <th className="px-4 py-2 text-left text-xs font-medium text-gray-400 uppercase">
                  Height
                </th>
                <th className="px-4 py-2 text-left text-xs font-medium text-gray-400 uppercase">
                  Hash
                </th>
                <th className="px-4 py-2 text-left text-xs font-medium text-gray-400 uppercase">
                  Timestamp
                </th>
                <th className="px-4 py-2 text-right text-xs font-medium text-gray-400 uppercase">
                  Transactions
                </th>
              </tr>
            </thead>
            <tbody className="divide-y divide-gray-800">
              {latestBlocks.map((block) => (
                <tr key={block.height} className="hover:bg-gray-800/30">
                  <td className="px-4 py-3">
                    <Link
                      to={`/block/${block.height}`}
                      className="text-bitcoin-orange hover:underline font-mono"
                    >
                      {block.height.toLocaleString()}
                    </Link>
                  </td>
                  <td className="px-4 py-3">
                    <Link
                      to={`/block/${block.hash}`}
                      className="text-gray-300 hover:text-bitcoin-orange font-mono text-sm"
                    >
                      {formatHash(block.hash)}
                    </Link>
                  </td>
                  <td className="px-4 py-3 text-gray-400 text-sm">
                    {formatTimestamp(block.timestamp)}
                  </td>
                  <td className="px-4 py-3 text-right text-gray-300">
                    {block.txCount}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>
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
