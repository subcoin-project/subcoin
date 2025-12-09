import { useMemo } from "react";
import {
  AreaChart,
  Area,
  XAxis,
  YAxis,
  Tooltip,
  ResponsiveContainer,
} from "recharts";
import type { AddressTransaction } from "@subcoin/shared";

interface BalanceHistoryChartProps {
  transactions: AddressTransaction[];
  currentBalance: number;
  totalTxCount: number;
}

interface DataPoint {
  timestamp: number;
  date: string;
  balance: number;
  balanceBtc: number;
  delta: number;
}

function formatBtc(satoshis: number): string {
  return (satoshis / 100_000_000).toFixed(8);
}

function formatShortBtc(satoshis: number): string {
  const btc = satoshis / 100_000_000;
  if (btc >= 1000) return `${(btc / 1000).toFixed(0)}k`;
  if (btc >= 100) return btc.toFixed(0);
  if (btc >= 10) return btc.toFixed(1);
  if (btc >= 1) return btc.toFixed(2);
  if (btc >= 0.01) return btc.toFixed(3);
  return btc.toFixed(4);
}

function formatDate(timestamp: number): string {
  return new Date(timestamp * 1000).toLocaleDateString();
}

export function BalanceHistoryChart({
  transactions,
  currentBalance,
  totalTxCount,
}: BalanceHistoryChartProps) {
  const hasAllTransactions = transactions.length >= totalTxCount;

  const dataPoints = useMemo(() => {
    if (transactions.length === 0) return [];

    // Sort by block_height ascending (oldest first), then by delta (receives before sends)
    const sorted = [...transactions].sort((a, b) => {
      if (a.block_height !== b.block_height) {
        return a.block_height - b.block_height;
      }
      // Within the same block, process receives (positive delta) before sends (negative delta)
      return b.delta - a.delta;
    });

    // Calculate balance history:
    // - currentBalance is the balance NOW (after all transactions)
    // - We work backwards to find balance before each transaction
    // - Then display balance AFTER each transaction

    const points: DataPoint[] = [];

    // Calculate total delta from all visible transactions
    const totalDelta = sorted.reduce((sum, tx) => sum + tx.delta, 0);

    // Balance before any of these transactions
    const startingBalance = currentBalance - totalDelta;

    // Add starting point (balance before first transaction)
    if (hasAllTransactions && sorted[0].timestamp > 0) {
      points.push({
        timestamp: sorted[0].timestamp - 86400,
        date: formatDate(sorted[0].timestamp - 86400),
        balance: startingBalance,
        balanceBtc: startingBalance / 100_000_000,
        delta: 0,
      });
    }

    // Process each transaction and show balance AFTER each one
    let runningBalance = startingBalance;
    for (const tx of sorted) {
      runningBalance += tx.delta;
      points.push({
        timestamp: tx.timestamp,
        date: formatDate(tx.timestamp),
        balance: runningBalance,
        balanceBtc: runningBalance / 100_000_000,
        delta: tx.delta,
      });
    }

    return points;
  }, [transactions, currentBalance, hasAllTransactions]);

  if (dataPoints.length < 2) {
    return (
      <div className="bg-bitcoin-dark rounded-lg border border-gray-800 p-4">
        <h3 className="text-sm font-medium text-gray-100 mb-2">
          Balance History
        </h3>
        <div className="text-gray-500 text-sm text-center py-4">
          Not enough data to display chart
        </div>
      </div>
    );
  }

  // Calculate Y-axis domain - let recharts handle it but ensure min is 0
  const maxBalance = Math.max(...dataPoints.map((d) => d.balanceBtc));
  const yDomain: [number, number] = [0, maxBalance * 1.1];

  return (
    <div className="bg-bitcoin-dark rounded-lg border border-gray-800 px-3 py-2">
      <div className="flex items-center justify-between mb-2">
        <h3 className="text-sm font-medium text-gray-100">Balance History</h3>
        {!hasAllTransactions && (
          <span className="text-xs text-gray-500">
            (showing {transactions.length} of {totalTxCount} txs)
          </span>
        )}
      </div>
      <div className="h-24">
        <ResponsiveContainer width="100%" height="100%">
          <AreaChart
            data={dataPoints}
            margin={{ top: 5, right: 5, left: 0, bottom: 0 }}
          >
            <defs>
              <linearGradient id="balanceGradient" x1="0" y1="0" x2="0" y2="1">
                <stop offset="5%" stopColor="#fb923c" stopOpacity={0.3} />
                <stop offset="95%" stopColor="#fb923c" stopOpacity={0.05} />
              </linearGradient>
            </defs>
            <XAxis
              dataKey="date"
              tick={{ fontSize: 9, fill: "#6b7280" }}
              tickLine={false}
              axisLine={{ stroke: "#374151" }}
              interval="preserveStartEnd"
            />
            <YAxis
              tick={{ fontSize: 9, fill: "#6b7280" }}
              tickLine={false}
              axisLine={false}
              tickFormatter={(value) => formatShortBtc(value * 100_000_000)}
              width={35}
              domain={yDomain}
            />
            <Tooltip
              contentStyle={{
                backgroundColor: "#1f2937",
                border: "1px solid #374151",
                borderRadius: "6px",
                fontSize: "12px",
              }}
              labelStyle={{ color: "#9ca3af" }}
              formatter={(value: number) => {
                return [`${formatBtc(value * 100_000_000)} BTC`, "Balance"];
              }}
            />
            <Area
              type="monotone"
              dataKey="balanceBtc"
              stroke="#fb923c"
              strokeWidth={1.5}
              fill="url(#balanceGradient)"
            />
          </AreaChart>
        </ResponsiveContainer>
      </div>
    </div>
  );
}
