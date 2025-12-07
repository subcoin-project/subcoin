import { useEffect, useState } from "react";
import { useParams } from "react-router-dom";
import { blockchainApi } from "@subcoin/shared";
import type { Transaction } from "@subcoin/shared";

export function TransactionDetail() {
  const { txid } = useParams<{ txid: string }>();
  const [transaction, setTransaction] = useState<Transaction | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    async function fetchTransaction() {
      if (!txid) return;

      try {
        setLoading(true);
        setError(null);

        const tx = await blockchainApi.getTransaction(txid);
        setTransaction(tx);
      } catch (err) {
        setError(err instanceof Error ? err.message : "Failed to fetch transaction");
      } finally {
        setLoading(false);
      }
    }

    fetchTransaction();
  }, [txid]);

  if (loading) {
    return (
      <div className="flex items-center justify-center h-64">
        <div className="text-gray-400">Loading transaction...</div>
      </div>
    );
  }

  if (error || !transaction) {
    return (
      <div className="bg-red-900/20 border border-red-800 rounded-lg p-4">
        <h3 className="text-red-400 font-medium">Error</h3>
        <p className="text-red-300 text-sm mt-1">{error || "Transaction not found"}</p>
      </div>
    );
  }

  const isCoinbase =
    transaction.input.length === 1 &&
    transaction.input[0].previous_output.txid ===
      "0000000000000000000000000000000000000000000000000000000000000000";

  const totalOutput = transaction.output.reduce((sum, out) => sum + out.value, 0);

  return (
    <div className="space-y-6">
      {/* Transaction Header */}
      <div className="bg-bitcoin-dark rounded-lg border border-gray-800 p-4">
        <div className="flex items-center space-x-3 mb-4">
          <h1 className="text-xl font-bold text-gray-100">Transaction</h1>
          {isCoinbase && (
            <span className="bg-bitcoin-orange/20 text-bitcoin-orange text-sm px-2 py-1 rounded">
              Coinbase
            </span>
          )}
        </div>

        <dl className="grid grid-cols-1 md:grid-cols-2 gap-4">
          <div className="md:col-span-2">
            <dt className="text-gray-400 text-sm">Transaction ID</dt>
            <dd className="text-gray-100 font-mono text-sm break-all mt-1">
              {txid}
            </dd>
          </div>
          <div>
            <dt className="text-gray-400 text-sm">Version</dt>
            <dd className="text-gray-100 mt-1">{transaction.version}</dd>
          </div>
          <div>
            <dt className="text-gray-400 text-sm">Lock Time</dt>
            <dd className="text-gray-100 mt-1">{transaction.lock_time}</dd>
          </div>
          <div>
            <dt className="text-gray-400 text-sm">Inputs</dt>
            <dd className="text-gray-100 mt-1">{transaction.input.length}</dd>
          </div>
          <div>
            <dt className="text-gray-400 text-sm">Outputs</dt>
            <dd className="text-gray-100 mt-1">{transaction.output.length}</dd>
          </div>
          <div>
            <dt className="text-gray-400 text-sm">Total Output</dt>
            <dd className="text-gray-100 font-mono mt-1">
              {(totalOutput / 100_000_000).toFixed(8)} BTC
            </dd>
          </div>
        </dl>
      </div>

      {/* Inputs */}
      <div className="bg-bitcoin-dark rounded-lg border border-gray-800">
        <div className="px-4 py-3 border-b border-gray-800">
          <h2 className="text-lg font-medium text-gray-100">
            Inputs ({transaction.input.length})
          </h2>
        </div>
        <div className="divide-y divide-gray-800">
          {transaction.input.map((input, index) => (
            <div key={index} className="px-4 py-3">
              <div className="flex items-center justify-between mb-2">
                <span className="text-gray-500 text-sm">#{index}</span>
                {isCoinbase && index === 0 && (
                  <span className="text-bitcoin-orange text-sm">Coinbase</span>
                )}
              </div>
              {!isCoinbase && (
                <>
                  <div className="text-gray-400 text-sm">Previous Output</div>
                  <div className="font-mono text-sm text-gray-300 break-all">
                    {input.previous_output.txid}:{input.previous_output.vout}
                  </div>
                </>
              )}
              <div className="mt-2">
                <div className="text-gray-400 text-sm">Script Sig</div>
                <div className="font-mono text-xs text-gray-500 break-all mt-1">
                  {input.script_sig || "(empty)"}
                </div>
              </div>
              <div className="mt-2 text-gray-500 text-sm">
                Sequence: {input.sequence}
              </div>
            </div>
          ))}
        </div>
      </div>

      {/* Outputs */}
      <div className="bg-bitcoin-dark rounded-lg border border-gray-800">
        <div className="px-4 py-3 border-b border-gray-800">
          <h2 className="text-lg font-medium text-gray-100">
            Outputs ({transaction.output.length})
          </h2>
        </div>
        <div className="divide-y divide-gray-800">
          {transaction.output.map((output, index) => (
            <div key={index} className="px-4 py-3">
              <div className="flex items-center justify-between mb-2">
                <span className="text-gray-500 text-sm">#{index}</span>
                <span className="font-mono text-bitcoin-orange">
                  {(output.value / 100_000_000).toFixed(8)} BTC
                </span>
              </div>
              <div className="text-gray-400 text-sm">Script PubKey</div>
              <div className="font-mono text-xs text-gray-500 break-all mt-1">
                {output.script_pubkey}
              </div>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
