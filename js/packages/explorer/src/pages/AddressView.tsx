import { useEffect, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { addressApi } from "@subcoin/shared";
import type {
  AddressBalance,
  AddressStats,
  AddressTransaction,
  AddressUtxo,
} from "@subcoin/shared";
import { AddressDetailSkeleton } from "../components/Skeleton";
import { CopyButton } from "../components/CopyButton";
import { ScriptTypeBadge } from "../components/ScriptTypeBadge";

type TabType = "transactions" | "utxos";

function formatBtc(satoshis: number): string {
  return (satoshis / 100_000_000).toFixed(8);
}

function formatDate(timestamp: number): string {
  if (timestamp === 0) return "Unknown";
  return new Date(timestamp * 1000).toLocaleString();
}

export function AddressView() {
  const { address } = useParams<{ address: string }>();
  const [balance, setBalance] = useState<AddressBalance | null>(null);
  const [stats, setStats] = useState<AddressStats | null>(null);
  const [statsExpanded, setStatsExpanded] = useState(false);
  const [statsLoading, setStatsLoading] = useState(false);
  const [transactions, setTransactions] = useState<AddressTransaction[]>([]);
  const [utxos, setUtxos] = useState<AddressUtxo[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [activeTab, setActiveTab] = useState<TabType>("transactions");
  const [page, setPage] = useState(0);
  const pageSize = 25;

  useEffect(() => {
    async function fetchAddressData() {
      if (!address) return;

      try {
        setLoading(true);
        setError(null);

        const [balanceData, historyData] = await Promise.all([
          addressApi.getBalance(address),
          addressApi.getHistory(address, pageSize, 0),
        ]);

        setBalance(balanceData);
        setTransactions(historyData);
      } catch (err) {
        setError(
          err instanceof Error ? err.message : "Failed to fetch address data"
        );
      } finally {
        setLoading(false);
      }
    }

    fetchAddressData();
  }, [address]);

  // Load stats when expanded
  useEffect(() => {
    if (!statsExpanded || stats || !address) return;

    async function loadStats() {
      try {
        setStatsLoading(true);
        const statsData = await addressApi.getStats(address!);
        setStats(statsData);
      } catch (err) {
        console.error("Failed to load stats:", err);
      } finally {
        setStatsLoading(false);
      }
    }

    loadStats();
  }, [statsExpanded, stats, address]);

  // Load more transactions when page changes
  useEffect(() => {
    if (page === 0 || !address) return;

    async function loadMoreTransactions() {
      try {
        const historyData = await addressApi.getHistory(
          address!,
          pageSize,
          page * pageSize
        );
        setTransactions((prev) => [...prev, ...historyData]);
      } catch (err) {
        console.error("Failed to load more transactions:", err);
      }
    }

    loadMoreTransactions();
  }, [page, address]);

  // Load UTXOs when switching to UTXOs tab
  useEffect(() => {
    if (activeTab !== "utxos" || !address || utxos.length > 0) return;

    async function loadUtxos() {
      try {
        const utxoData = await addressApi.getUtxos(address!);
        setUtxos(utxoData);
      } catch (err) {
        console.error("Failed to load UTXOs:", err);
      }
    }

    loadUtxos();
  }, [activeTab, address, utxos.length]);

  if (loading) {
    return <AddressDetailSkeleton />;
  }

  if (error || !balance) {
    return (
      <div className="bg-red-900/20 border border-red-800 rounded-lg p-4">
        <h3 className="text-red-400 font-medium">Error</h3>
        <p className="text-red-300 text-sm mt-1">
          {error || "Address data not available. Is the indexer enabled?"}
        </p>
      </div>
    );
  }

  return (
    <div className="space-y-6">
      {/* Address Header */}
      <div className="bg-bitcoin-dark rounded-lg border border-gray-800 p-4">
        <h1 className="text-xl font-bold text-gray-100 mb-4">Address</h1>

        <div className="mb-4">
          <dt className="text-gray-400 text-sm">Address</dt>
          <dd className="text-gray-100 font-mono text-sm break-all mt-1 flex items-start gap-1">
            <span className="flex-1">{address}</span>
            <CopyButton text={address || ""} />
          </dd>
        </div>

        {/* Balance Stats */}
        <div className="grid grid-cols-2 md:grid-cols-5 gap-4">
          <div>
            <dt className="text-gray-400 text-sm">Balance</dt>
            <dd className="text-bitcoin-orange font-mono text-lg mt-1">
              {formatBtc(balance.confirmed)} BTC
            </dd>
          </div>
          <div>
            <dt className="text-gray-400 text-sm">Total Received</dt>
            <dd className="text-green-400 font-mono mt-1">
              {formatBtc(balance.total_received)} BTC
            </dd>
          </div>
          <div>
            <dt className="text-gray-400 text-sm">Total Sent</dt>
            <dd className="text-red-400 font-mono mt-1">
              {formatBtc(balance.total_sent)} BTC
            </dd>
          </div>
          <div>
            <dt className="text-gray-400 text-sm">Transactions</dt>
            <dd className="text-gray-100 mt-1">{balance.tx_count}</dd>
          </div>
          <div>
            <dt className="text-gray-400 text-sm">UTXOs</dt>
            <dd className="text-gray-100 mt-1">{balance.utxo_count}</dd>
          </div>
        </div>
      </div>

      {/* Address Statistics (collapsible) */}
      <div className="bg-bitcoin-dark rounded-lg border border-gray-800">
        <button
          onClick={() => setStatsExpanded(!statsExpanded)}
          className="w-full px-4 py-3 flex items-center justify-between text-left hover:bg-gray-800/50 transition-colors"
        >
          <h2 className="text-lg font-medium text-gray-100">Statistics</h2>
          <span className="text-gray-400 text-sm">
            {statsExpanded ? "▼" : "▶"}
          </span>
        </button>

        {statsExpanded && (
          <div className="px-4 pb-4 border-t border-gray-800">
            {statsLoading ? (
              <div className="py-4 text-center text-gray-400">
                Loading statistics...
              </div>
            ) : stats ? (
              <div className="grid grid-cols-2 md:grid-cols-4 gap-4 pt-4">
                <div>
                  <dt className="text-gray-400 text-sm">First Seen</dt>
                  <dd className="text-gray-100 mt-1">
                    {stats.first_seen_height !== null ? (
                      <>
                        <Link
                          to={`/block/${stats.first_seen_height}`}
                          className="text-blue-400 hover:text-blue-300"
                        >
                          Block #{stats.first_seen_height}
                        </Link>
                        <div className="text-xs text-gray-500 mt-0.5">
                          {stats.first_seen_timestamp !== null
                            ? formatDate(stats.first_seen_timestamp)
                            : "Unknown"}
                        </div>
                      </>
                    ) : (
                      <span className="text-gray-500">Never</span>
                    )}
                  </dd>
                </div>
                <div>
                  <dt className="text-gray-400 text-sm">Last Seen</dt>
                  <dd className="text-gray-100 mt-1">
                    {stats.last_seen_height !== null ? (
                      <>
                        <Link
                          to={`/block/${stats.last_seen_height}`}
                          className="text-blue-400 hover:text-blue-300"
                        >
                          Block #{stats.last_seen_height}
                        </Link>
                        <div className="text-xs text-gray-500 mt-0.5">
                          {stats.last_seen_timestamp !== null
                            ? formatDate(stats.last_seen_timestamp)
                            : "Unknown"}
                        </div>
                      </>
                    ) : (
                      <span className="text-gray-500">Never</span>
                    )}
                  </dd>
                </div>
                <div>
                  <dt className="text-gray-400 text-sm">Largest Receive</dt>
                  <dd className="text-green-400 font-mono mt-1">
                    {formatBtc(stats.largest_receive)} BTC
                  </dd>
                  <div className="text-xs text-gray-500 mt-0.5">
                    {stats.receive_count} receive txs
                  </div>
                </div>
                <div>
                  <dt className="text-gray-400 text-sm">Largest Send</dt>
                  <dd className="text-red-400 font-mono mt-1">
                    {formatBtc(stats.largest_send)} BTC
                  </dd>
                  <div className="text-xs text-gray-500 mt-0.5">
                    {stats.send_count} send txs
                  </div>
                </div>
              </div>
            ) : (
              <div className="py-4 text-center text-gray-500">
                Failed to load statistics
              </div>
            )}
          </div>
        )}
      </div>

      {/* Tabs */}
      <div className="flex border-b border-gray-800">
        <button
          className={`px-4 py-2 text-sm font-medium ${
            activeTab === "transactions"
              ? "text-bitcoin-orange border-b-2 border-bitcoin-orange"
              : "text-gray-400 hover:text-gray-200"
          }`}
          onClick={() => setActiveTab("transactions")}
        >
          Transactions ({balance.tx_count})
        </button>
        <button
          className={`px-4 py-2 text-sm font-medium ${
            activeTab === "utxos"
              ? "text-bitcoin-orange border-b-2 border-bitcoin-orange"
              : "text-gray-400 hover:text-gray-200"
          }`}
          onClick={() => setActiveTab("utxos")}
        >
          UTXOs ({balance.utxo_count})
        </button>
      </div>

      {/* Transaction History */}
      {activeTab === "transactions" && (
        <div className="bg-bitcoin-dark rounded-lg border border-gray-800">
          <div className="px-4 py-3 border-b border-gray-800">
            <h2 className="text-lg font-medium text-gray-100">
              Transaction History
            </h2>
          </div>
          {transactions.length === 0 ? (
            <div className="px-4 py-8 text-center text-gray-400">
              No transactions found
            </div>
          ) : (
            <div className="divide-y divide-gray-800">
              {transactions.map((tx) => (
                <div key={tx.txid} className="px-4 py-3 hover:bg-gray-800/50">
                  <div className="flex items-center justify-between mb-2">
                    <Link
                      to={`/tx/${tx.txid}`}
                      className="font-mono text-sm text-blue-400 hover:text-blue-300 truncate max-w-[70%]"
                    >
                      {tx.txid}
                    </Link>
                    <span
                      className={`font-mono text-sm ${
                        tx.delta >= 0 ? "text-green-400" : "text-red-400"
                      }`}
                    >
                      {tx.delta >= 0 ? "+" : ""}
                      {formatBtc(tx.delta)} BTC
                    </span>
                  </div>
                  <div className="flex items-center justify-between text-sm text-gray-400">
                    <Link
                      to={`/block/${tx.block_height}`}
                      className="hover:text-gray-200"
                    >
                      Block #{tx.block_height}
                    </Link>
                    <span>{formatDate(tx.timestamp)}</span>
                  </div>
                </div>
              ))}
            </div>
          )}

          {/* Load more button */}
          {transactions.length < balance.tx_count && (
            <div className="px-4 py-3 border-t border-gray-800">
              <button
                onClick={() => setPage((p) => p + 1)}
                className="w-full py-2 text-sm text-bitcoin-orange hover:text-bitcoin-orange/80"
              >
                Load more transactions...
              </button>
            </div>
          )}
        </div>
      )}

      {/* UTXOs */}
      {activeTab === "utxos" && (
        <div className="bg-bitcoin-dark rounded-lg border border-gray-800">
          <div className="px-4 py-3 border-b border-gray-800">
            <h2 className="text-lg font-medium text-gray-100">
              Unspent Transaction Outputs
            </h2>
          </div>
          {utxos.length === 0 ? (
            <div className="px-4 py-8 text-center text-gray-400">
              {balance.utxo_count === 0
                ? "No UTXOs found"
                : "Loading UTXOs..."}
            </div>
          ) : (
            <div className="divide-y divide-gray-800">
              {utxos.map((utxo) => (
                <div
                  key={`${utxo.txid}:${utxo.vout}`}
                  className="px-4 py-3 hover:bg-gray-800/50"
                >
                  <div className="flex items-center justify-between mb-2">
                    <div className="flex items-center gap-2">
                      <Link
                        to={`/tx/${utxo.txid}`}
                        className="font-mono text-sm text-blue-400 hover:text-blue-300 truncate max-w-[50%]"
                      >
                        {utxo.txid}
                      </Link>
                      <span className="text-gray-500 text-sm">
                        :{utxo.vout}
                      </span>
                      <ScriptTypeBadge scriptPubkey={utxo.script_pubkey} />
                    </div>
                    <span className="font-mono text-sm text-green-400">
                      {formatBtc(utxo.value)} BTC
                    </span>
                  </div>
                  <div className="flex items-center justify-between text-sm text-gray-400">
                    <Link
                      to={`/block/${utxo.block_height}`}
                      className="hover:text-gray-200"
                    >
                      Block #{utxo.block_height}
                    </Link>
                  </div>
                </div>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
