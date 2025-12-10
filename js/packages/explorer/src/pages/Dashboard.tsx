import { useEffect, useState, useCallback } from "react";
import { Link } from "react-router-dom";
import { addressApi, blockchainApi, networkApi, systemApi } from "@subcoin/shared";
import type { IndexerStatus, NetworkStatus } from "@subcoin/shared";
import { useNewBlockSubscription, useConnection } from "../contexts/ConnectionContext";
import { StatCardSkeleton, BlockTableSkeleton } from "../components/Skeleton";

interface BlockInfo {
  height: number;
  hash: string;
  timestamp: number;
  txCount: number;
  /** Miner address (first coinbase output recipient) */
  minerAddress: string | null;
  /** Block reward in satoshis (sum of coinbase outputs) */
  reward: number;
}

export function Dashboard() {
  const [latestBlocks, setLatestBlocks] = useState<BlockInfo[]>([]);
  const [networkStatus, setNetworkStatus] = useState<NetworkStatus | null>(null);
  const [syncState, setSyncState] = useState<{ current: number; highest?: number } | null>(null);
  const [indexerStatus, setIndexerStatus] = useState<IndexerStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [lastUpdate, setLastUpdate] = useState<Date | null>(null);
  const { isConnected } = useConnection();

  const fetchData = useCallback(async () => {
    try {
      setLoading(true);
      setError(null);

      // Fetch network status and indexer status
      const [status, idxStatus] = await Promise.all([
        networkApi.getStatus(),
        addressApi.getIndexerStatus().catch(() => null),
      ]);
      setNetworkStatus(status);
      setIndexerStatus(idxStatus);

      // Fetch sync state from system API
      const sync = await systemApi.syncState();

      // Get sync target from multiple sources (prefer network status target)
      let highestBlock = sync.highestBlock;

      // Try to get from syncStatus target (more accurate during active sync)
      // RPC returns { downloading: { target, peers } } or { importing: { target, peers } }
      if (status?.syncStatus) {
        if ("downloading" in status.syncStatus && status.syncStatus.downloading) {
          highestBlock = status.syncStatus.downloading.target;
        } else if ("importing" in status.syncStatus && status.syncStatus.importing) {
          highestBlock = status.syncStatus.importing.target;
        }
      }

      // Fallback: try to get from sync peers
      if (!highestBlock || highestBlock === sync.currentBlock) {
        try {
          const syncPeers = await networkApi.getSyncPeers();
          if (syncPeers.bestKnownBlock) {
            highestBlock = syncPeers.bestKnownBlock;
          }
        } catch {
          // Ignore if this call fails
        }
      }

      setSyncState({ current: sync.currentBlock, highest: highestBlock });

      // Fetch latest blocks
      const blocks: BlockInfo[] = [];
      const currentHeight = sync.currentBlock;

      // Fetch last 10 blocks in parallel
      const blockPromises = [];
      for (let i = 0; i < 10 && currentHeight - i >= 0; i++) {
        const height = currentHeight - i;
        blockPromises.push(
          (async () => {
            const [block, hash] = await Promise.all([
              blockchainApi.getBlockByNumber(height),
              blockchainApi.getBlockHash(height),
            ]);
            if (block) {
              // Extract coinbase info (first transaction)
              const coinbaseTx = block.txdata[0];
              const reward = coinbaseTx
                ? coinbaseTx.output.reduce((sum, out) => sum + out.value, 0)
                : 0;

              // Get miner address from first coinbase output
              let minerAddress: string | null = null;
              if (coinbaseTx && coinbaseTx.output.length > 0) {
                const scriptPubkey = coinbaseTx.output[0].script_pubkey;
                minerAddress = await blockchainApi.decodeScriptPubkey(scriptPubkey);
              }

              return {
                height,
                hash: hash || "",
                timestamp: block.header.time,
                txCount: block.txdata.length,
                minerAddress,
                reward,
              };
            }
            return null;
          })()
        );
      }

      const results = await Promise.all(blockPromises);
      for (const result of results) {
        if (result) blocks.push(result);
      }

      // Sort by height descending (in case parallel fetch returned out of order)
      blocks.sort((a, b) => b.height - a.height);

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

  const formatRelativeTime = (timestamp: number) => {
    const now = Math.floor(Date.now() / 1000);
    const diff = now - timestamp;

    if (diff < 60) return `${diff}s ago`;
    if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
    if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
    return `${Math.floor(diff / 86400)}d ago`;
  };

  const formatAddress = (address: string) => {
    if (!address) return "";
    if (address.length <= 16) return address;
    return `${address.slice(0, 8)}...${address.slice(-6)}`;
  };

  const formatBtc = (satoshis: number) => {
    return (satoshis / 100_000_000).toFixed(8);
  };

  const isInitialLoading = loading && latestBlocks.length === 0;

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
      <div className="grid grid-cols-2 md:grid-cols-4 gap-3 md:gap-4">
        {isInitialLoading ? (
          <>
            <StatCardSkeleton />
            <StatCardSkeleton />
            <StatCardSkeleton />
            <StatCardSkeleton />
          </>
        ) : (
          <>
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
              title={indexerStatus?.is_syncing ? "Indexer" : "Network"}
              value={
                indexerStatus?.is_syncing
                  ? `${indexerStatus.progress_percent.toFixed(1)}%`
                  : getSyncStatusText(networkStatus?.syncStatus)
              }
              subtitle={
                indexerStatus?.is_syncing
                  ? `${indexerStatus.indexed_height.toLocaleString()} / ${indexerStatus.target_height?.toLocaleString() ?? "?"}`
                  : undefined
              }
            />
          </>
        )}
      </div>

      {/* Latest Blocks - Card Style */}
      <div className="bg-bitcoin-dark rounded-lg border border-gray-800">
        <div className="px-4 py-3 border-b border-gray-800 flex items-center justify-between">
          <h2 className="text-lg font-medium text-gray-100">Recent Blocks</h2>
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

        {isInitialLoading ? (
          <BlockTableSkeleton rows={10} />
        ) : (
          <div className="divide-y divide-gray-800">
            {latestBlocks.map((block, index) => (
              <div
                key={block.height}
                className={`px-4 py-3 hover:bg-gray-800/30 transition-colors ${
                  index === 0 ? "bg-bitcoin-orange/5" : ""
                }`}
              >
                <div className="flex items-start justify-between gap-4">
                  {/* Left: Block info */}
                  <div className="flex items-start gap-3 min-w-0 flex-1">
                    {/* Block number badge */}
                    <Link
                      to={`/block/${block.height}`}
                      className="flex-shrink-0 bg-gray-800 hover:bg-gray-700 rounded px-2.5 py-1 text-bitcoin-orange font-mono text-sm font-medium transition-colors"
                    >
                      #{block.height.toLocaleString()}
                    </Link>

                    {/* Miner info */}
                    <div className="min-w-0 flex-1">
                      <div className="flex items-center gap-2 flex-wrap">
                        {block.minerAddress ? (
                          <Link
                            to={`/address/${block.minerAddress}`}
                            className="text-blue-400 hover:text-blue-300 font-mono text-sm truncate"
                            title={block.minerAddress}
                          >
                            {formatAddress(block.minerAddress)}
                          </Link>
                        ) : (
                          <span className="text-gray-500 text-sm italic">
                            Non-standard output
                          </span>
                        )}
                        <span className="text-green-400 text-xs font-mono">
                          +{formatBtc(block.reward)} BTC
                        </span>
                      </div>
                      <div className="flex items-center gap-3 mt-1 text-xs text-gray-500">
                        <span>{block.txCount} txs</span>
                        <span className="hidden sm:inline">•</span>
                        <span className="hidden sm:inline font-mono" title={block.hash}>
                          {block.hash.slice(0, 12)}...
                        </span>
                      </div>
                    </div>
                  </div>

                  {/* Right: Time */}
                  <div className="flex-shrink-0 text-right">
                    <span className="text-gray-400 text-sm italic">
                      {formatRelativeTime(block.timestamp)}
                    </span>
                  </div>
                </div>
              </div>
            ))}
          </div>
        )}

        <div className="px-4 py-3 border-t border-gray-800">
          <Link
            to="/blocks"
            className="text-bitcoin-orange hover:underline text-sm"
          >
            View all blocks &rarr;
          </Link>
        </div>
      </div>
    </div>
  );
}

function StatCard({
  title,
  value,
  subtitle,
}: {
  title: string;
  value: string;
  subtitle?: string;
}) {
  return (
    <div className="bg-bitcoin-dark rounded-lg border border-gray-800 p-3 md:p-4">
      <p className="text-gray-400 text-xs md:text-sm truncate">{title}</p>
      <p className="text-lg md:text-2xl font-bold text-gray-100 mt-1 truncate">{value}</p>
      {subtitle && (
        <p className="text-gray-500 text-xs mt-0.5 truncate">{subtitle}</p>
      )}
    </div>
  );
}

function getSyncStatusText(status: NetworkStatus["syncStatus"] | undefined): string {
  if (!status) return "-";
  if ("idle" in status) return "Idle";
  if ("downloading" in status) return "Downloading";
  if ("importing" in status) return "Importing";
  return "Unknown";
}
