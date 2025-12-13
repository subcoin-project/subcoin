import { useEffect, useState, useCallback } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { blockchainApi, systemApi } from "@subcoin/shared";
import { BlockTableSkeleton } from "../components/Skeleton";

interface BlockInfo {
  height: number;
  hash: string;
  timestamp: number;
  txCount: number;
}

const BLOCKS_PER_PAGE = 25;

export function BlockList() {
  const [searchParams, setSearchParams] = useSearchParams();
  const [blocks, setBlocks] = useState<BlockInfo[]>([]);
  const [currentHeight, setCurrentHeight] = useState<number | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  // Get page from URL params, default to 1
  const page = parseInt(searchParams.get("page") || "1", 10);

  const fetchBlocks = useCallback(async () => {
    try {
      setLoading(true);
      setError(null);

      // Get current chain height
      const sync = await systemApi.syncState();
      const chainHeight = sync.currentBlock;
      setCurrentHeight(chainHeight);

      // Calculate block range for this page
      const startHeight = chainHeight - (page - 1) * BLOCKS_PER_PAGE;
      const endHeight = Math.max(0, startHeight - BLOCKS_PER_PAGE + 1);

      const fetchedBlocks: BlockInfo[] = [];

      for (let height = startHeight; height >= endHeight && height >= 0; height--) {
        const block = await blockchainApi.getBlockByNumber(height);
        if (block) {
          const hash = await blockchainApi.getBlockHash(height);
          fetchedBlocks.push({
            height,
            hash: hash || "",
            timestamp: block.header.time,
            txCount: block.txdata.length,
          });
        }
      }

      setBlocks(fetchedBlocks);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to fetch blocks");
    } finally {
      setLoading(false);
    }
  }, [page]);

  useEffect(() => {
    fetchBlocks();
  }, [fetchBlocks]);

  const totalPages = currentHeight !== null ? Math.ceil((currentHeight + 1) / BLOCKS_PER_PAGE) : 1;

  const goToPage = (newPage: number) => {
    if (newPage >= 1 && newPage <= totalPages) {
      setSearchParams({ page: newPage.toString() });
    }
  };

  const formatTimestamp = (timestamp: number) => {
    return new Date(timestamp * 1000).toLocaleString();
  };

  const formatHash = (hash: string) => {
    if (!hash) return "";
    return `${hash.slice(0, 8)}...${hash.slice(-8)}`;
  };

  const formatTimeAgo = (timestamp: number) => {
    const now = Date.now() / 1000;
    const diff = now - timestamp;

    if (diff < 60) return `${Math.floor(diff)}s ago`;
    if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
    if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
    return `${Math.floor(diff / 86400)}d ago`;
  };

  if (error) {
    return (
      <div className="bg-red-900/20 border border-red-800 rounded-lg p-4">
        <h3 className="text-red-400 font-medium">Error</h3>
        <p className="text-red-300 text-sm mt-1">{error}</p>
      </div>
    );
  }

  return (
    <div className="space-y-4">
      {/* Header */}
      <div className="flex items-center justify-between">
        <h1 className="text-xl font-bold text-gray-100">Blocks</h1>
        {currentHeight !== null && (
          <span className="text-gray-400 text-sm">
            Total: {(currentHeight + 1).toLocaleString()} blocks
          </span>
        )}
      </div>

      {/* Block Table */}
      <div className="bg-bitcoin-dark rounded-lg border border-gray-800">
        <div className="overflow-x-auto">
          <table className="w-full">
            <thead className="bg-gray-800/50">
              <tr>
                <th className="px-4 py-3 text-left text-xs font-medium text-gray-400 uppercase">
                  Height
                </th>
                <th className="px-4 py-3 text-left text-xs font-medium text-gray-400 uppercase hidden sm:table-cell">
                  Hash
                </th>
                <th className="px-4 py-3 text-left text-xs font-medium text-gray-400 uppercase hidden lg:table-cell">
                  Timestamp
                </th>
                <th className="px-4 py-3 text-left text-xs font-medium text-gray-400 uppercase">
                  Age
                </th>
                <th className="px-4 py-3 text-right text-xs font-medium text-gray-400 uppercase">
                  Txs
                </th>
              </tr>
            </thead>
            {loading ? (
              <BlockTableSkeleton rows={BLOCKS_PER_PAGE} />
            ) : (
              <tbody className="divide-y divide-gray-800">
                {blocks.map((block) => (
                  <tr key={block.height} className="hover:bg-gray-800/30">
                    <td className="px-4 py-3">
                      <Link
                        to={`/block/${block.height}`}
                        className="text-bitcoin-orange hover:underline font-mono text-sm"
                      >
                        {block.height.toLocaleString()}
                      </Link>
                    </td>
                    <td className="px-4 py-3 hidden sm:table-cell">
                      <Link
                        to={`/block/${block.hash}`}
                        className="text-gray-300 hover:text-bitcoin-orange font-mono text-sm"
                      >
                        {formatHash(block.hash)}
                      </Link>
                    </td>
                    <td className="px-4 py-3 text-gray-400 text-sm hidden lg:table-cell">
                      {formatTimestamp(block.timestamp)}
                    </td>
                    <td className="px-4 py-3 text-gray-400 text-sm">
                      {formatTimeAgo(block.timestamp)}
                    </td>
                    <td className="px-4 py-3 text-right text-gray-300">
                      {block.txCount}
                    </td>
                  </tr>
                ))}
              </tbody>
            )}
          </table>
        </div>

        {/* Pagination */}
        <div className="px-4 py-3 border-t border-gray-800 flex items-center justify-between">
          <div className="text-gray-400 text-sm">
            <span className="hidden sm:inline">
              Showing {blocks.length > 0 ? blocks[0].height.toLocaleString() : "-"} to {blocks.length > 0 ? blocks[blocks.length - 1].height.toLocaleString() : "-"} ({blocks.length} blocks) &middot;{" "}
            </span>
            Page {page}/{totalPages.toLocaleString()}
          </div>
          <div className="flex items-center space-x-2">
            <button
              onClick={() => goToPage(1)}
              disabled={page === 1}
              className="px-3 py-1 rounded text-sm bg-gray-700 text-gray-300 hover:bg-gray-600 disabled:opacity-50 disabled:cursor-not-allowed"
            >
              First
            </button>
            <button
              onClick={() => goToPage(page - 1)}
              disabled={page === 1}
              className="px-3 py-1 rounded text-sm bg-gray-700 text-gray-300 hover:bg-gray-600 disabled:opacity-50 disabled:cursor-not-allowed"
            >
              Prev
            </button>
            <button
              onClick={() => goToPage(page + 1)}
              disabled={page >= totalPages}
              className="px-3 py-1 rounded text-sm bg-gray-700 text-gray-300 hover:bg-gray-600 disabled:opacity-50 disabled:cursor-not-allowed"
            >
              Next
            </button>
            <button
              onClick={() => goToPage(totalPages)}
              disabled={page >= totalPages}
              className="px-3 py-1 rounded text-sm bg-gray-700 text-gray-300 hover:bg-gray-600 disabled:opacity-50 disabled:cursor-not-allowed"
            >
              Last
            </button>
          </div>
        </div>
      </div>

      {/* Quick Jump */}
      <div className="bg-bitcoin-dark rounded-lg border border-gray-800 p-4">
        <h3 className="text-gray-400 text-sm mb-2">Jump to block</h3>
        <div className="flex items-center space-x-2">
          <input
            type="number"
            min={0}
            max={currentHeight ?? 0}
            placeholder="Enter block height"
            className="flex-1 bg-gray-800 border border-gray-700 rounded px-3 py-2 text-gray-100 text-sm focus:outline-none focus:border-bitcoin-orange"
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                const height = parseInt((e.target as HTMLInputElement).value, 10);
                if (!isNaN(height) && height >= 0 && currentHeight !== null && height <= currentHeight) {
                  window.location.href = `/block/${height}`;
                }
              }
            }}
          />
          <button
            onClick={(e) => {
              const input = (e.target as HTMLElement).previousElementSibling as HTMLInputElement;
              const height = parseInt(input.value, 10);
              if (!isNaN(height) && height >= 0 && currentHeight !== null && height <= currentHeight) {
                window.location.href = `/block/${height}`;
              }
            }}
            className="px-4 py-2 bg-bitcoin-orange text-white rounded text-sm hover:bg-bitcoin-orange/80"
          >
            Go
          </button>
        </div>
      </div>
    </div>
  );
}
