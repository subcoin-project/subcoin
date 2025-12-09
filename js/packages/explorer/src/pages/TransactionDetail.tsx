import { useEffect, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { addressApi, blockchainApi, detectScriptType } from "@subcoin/shared";
import type { IndexerStatus, Transaction, TxIn } from "@subcoin/shared";
import { TransactionDetailSkeleton } from "../components/Skeleton";
import { CopyButton } from "../components/CopyButton";
import { ScriptTypeBadge, OpReturnData } from "../components/ScriptTypeBadge";
import { TransactionSankey } from "../components/TransactionSankey";

// RBF is signaled when any input has sequence < 0xfffffffe
const RBF_MAX_SEQUENCE = 0xfffffffe;

function isRbfEnabled(inputs: TxIn[]): boolean {
  return inputs.some((input) => input.sequence < RBF_MAX_SEQUENCE);
}

function hasWitnessData(inputs: TxIn[]): boolean {
  return inputs.some((input) => input.witness && input.witness.length > 0);
}

// Estimate transaction size (simplified - actual size depends on serialization)
function estimateTxSize(tx: Transaction): { size: number; vsize: number; weight: number } {
  // Base transaction overhead: version(4) + locktime(4) + input count(1-3) + output count(1-3)
  let baseSize = 10;
  let witnessSize = 0;
  const hasWitness = hasWitnessData(tx.input);

  // Inputs: each ~41 bytes base (outpoint 36 + sequence 4 + script length 1) + script_sig
  for (const input of tx.input) {
    baseSize += 41 + (input.script_sig ? input.script_sig.length / 2 : 0);
    if (input.witness) {
      // Witness data size
      for (const w of input.witness) {
        witnessSize += 1 + w.length / 2; // length prefix + data
      }
    }
  }

  // Outputs: each ~9 bytes base (value 8 + script length 1) + script_pubkey
  for (const output of tx.output) {
    baseSize += 9 + (output.script_pubkey ? output.script_pubkey.length / 2 : 0);
  }

  // Add witness marker/flag if has witness
  if (hasWitness) {
    baseSize += 2; // marker + flag
  }

  const size = baseSize + witnessSize;
  const weight = baseSize * 4 + witnessSize;
  const vsize = Math.ceil(weight / 4);

  return { size, vsize, weight };
}

// Component to decode and display address from script_pubkey
function OutputAddress({ scriptPubkey }: { scriptPubkey: string }) {
  const [address, setAddress] = useState<string | null | undefined>(undefined);

  useEffect(() => {
    async function decodeAddress() {
      try {
        const decoded = await blockchainApi.decodeScriptPubkey(scriptPubkey);
        setAddress(decoded);
      } catch {
        setAddress(null);
      }
    }
    decodeAddress();
  }, [scriptPubkey]);

  if (address === undefined) {
    return <span className="text-gray-500 text-sm">Decoding...</span>;
  }

  if (address === null) {
    return <span className="text-gray-500 text-sm">(non-standard)</span>;
  }

  return (
    <Link
      to={`/address/${address}`}
      className="text-blue-400 hover:text-blue-300 font-mono text-sm break-all"
    >
      {address}
    </Link>
  );
}

// Component to show when transaction is not found (with indexer status)
function TransactionNotFound({ txid, error }: { txid?: string; error: string | null }) {
  const [indexerStatus, setIndexerStatus] = useState<IndexerStatus | null>(null);
  const [statusLoading, setStatusLoading] = useState(true);

  useEffect(() => {
    async function fetchIndexerStatus() {
      try {
        const status = await addressApi.getIndexerStatus();
        setIndexerStatus(status);
      } catch {
        // Indexer might not be enabled
        setIndexerStatus(null);
      } finally {
        setStatusLoading(false);
      }
    }
    fetchIndexerStatus();
  }, []);

  return (
    <div className="space-y-4">
      <div className="bg-yellow-900/20 border border-yellow-800 rounded-lg p-4">
        <h3 className="text-yellow-400 font-medium">Transaction Not Found</h3>
        <p className="text-yellow-300 text-sm mt-1">
          {error || "The requested transaction could not be found."}
        </p>
        {txid && (
          <p className="text-gray-400 font-mono text-xs mt-2 break-all">
            TXID: {txid}
          </p>
        )}
      </div>

      {/* Indexer Status */}
      {!statusLoading && indexerStatus && (
        <div className="bg-bitcoin-dark rounded-lg border border-gray-800 p-4">
          <h3 className="text-gray-100 font-medium mb-3">Transaction Index Status</h3>

          {indexerStatus.is_syncing ? (
            <div className="space-y-3">
              <p className="text-gray-400 text-sm">
                The transaction index is currently syncing. This transaction may become available once indexing catches up.
              </p>

              <div className="space-y-2">
                <div className="flex justify-between text-sm">
                  <span className="text-gray-400">Progress</span>
                  <span className="text-gray-100">
                    {indexerStatus.progress_percent.toFixed(1)}%
                  </span>
                </div>

                {/* Progress bar */}
                <div className="w-full bg-gray-700 rounded-full h-2">
                  <div
                    className="bg-bitcoin-orange h-2 rounded-full transition-all duration-300"
                    style={{ width: `${Math.min(indexerStatus.progress_percent, 100)}%` }}
                  />
                </div>

                <div className="flex justify-between text-sm">
                  <span className="text-gray-400">Indexed Height</span>
                  <span className="text-gray-100 font-mono">
                    {indexerStatus.indexed_height.toLocaleString()}
                  </span>
                </div>

                {indexerStatus.target_height && (
                  <div className="flex justify-between text-sm">
                    <span className="text-gray-400">Target Height</span>
                    <span className="text-gray-100 font-mono">
                      {indexerStatus.target_height.toLocaleString()}
                    </span>
                  </div>
                )}
              </div>
            </div>
          ) : (
            <p className="text-gray-400 text-sm">
              The transaction index is fully synced (height {indexerStatus.indexed_height.toLocaleString()}).
              If this transaction exists, it should be in the index.
            </p>
          )}
        </div>
      )}
    </div>
  );
}

interface InputValueInfo {
  values: number[];
  totalInput: number;
}

export function TransactionDetail() {
  const { txid } = useParams<{ txid: string }>();
  const [transaction, setTransaction] = useState<Transaction | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [inputValues, setInputValues] = useState<InputValueInfo | null>(null);

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

  // Fetch input values from previous outputs to calculate fee
  useEffect(() => {
    async function fetchInputValues() {
      if (!transaction) return;

      // Skip for coinbase transactions
      const isCoinbase =
        transaction.input.length === 1 &&
        transaction.input[0].previous_output.txid ===
          "0000000000000000000000000000000000000000000000000000000000000000";
      if (isCoinbase) return;

      try {
        const values = await Promise.all(
          transaction.input.map(async (input) => {
            try {
              const prevTx = await blockchainApi.getTransaction(input.previous_output.txid);
              if (prevTx && prevTx.output[input.previous_output.vout]) {
                return prevTx.output[input.previous_output.vout].value;
              }
            } catch {
              // Ignore errors for individual inputs
            }
            return 0;
          })
        );

        const totalInput = values.reduce((sum, v) => sum + v, 0);
        setInputValues({ values, totalInput });
      } catch {
        // Ignore errors
      }
    }

    fetchInputValues();
  }, [transaction]);

  if (loading) {
    return <TransactionDetailSkeleton />;
  }

  if (error || !transaction) {
    return <TransactionNotFound txid={txid} error={error} />;
  }

  const isCoinbase =
    transaction.input.length === 1 &&
    transaction.input[0].previous_output.txid ===
      "0000000000000000000000000000000000000000000000000000000000000000";

  const totalOutput = transaction.output.reduce((sum, out) => sum + out.value, 0);
  const rbfEnabled = isRbfEnabled(transaction.input);
  const isSegWit = hasWitnessData(transaction.input);
  const txSize = estimateTxSize(transaction);

  return (
    <div className="space-y-6">
      {/* Transaction Header */}
      <div className="bg-bitcoin-dark rounded-lg border border-gray-800 p-4">
        <div className="flex items-center flex-wrap gap-2 mb-4">
          <h1 className="text-xl font-bold text-gray-100">Transaction</h1>
          {isCoinbase && (
            <span className="bg-bitcoin-orange/20 text-bitcoin-orange text-sm px-2 py-1 rounded">
              Coinbase
            </span>
          )}
          <span
            className={`text-sm px-2 py-1 rounded ${
              isSegWit
                ? "bg-purple-500/20 text-purple-400"
                : "bg-gray-700/50 text-gray-500"
            }`}
          >
            {isSegWit ? "SegWit" : "Legacy"}
          </span>
          <span
            className={`text-sm px-2 py-1 rounded ${
              rbfEnabled
                ? "bg-yellow-500/20 text-yellow-400"
                : "bg-gray-700/50 text-gray-500"
            }`}
            title={rbfEnabled ? "Replace-By-Fee enabled" : "Replace-By-Fee disabled"}
          >
            RBF {rbfEnabled ? "✓" : "✗"}
          </span>
        </div>

        <dl className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
          <div className="md:col-span-2 lg:col-span-3">
            <dt className="text-gray-400 text-sm">Transaction ID</dt>
            <dd className="text-gray-100 font-mono text-sm break-all mt-1 flex items-start gap-1">
              <span className="flex-1">{txid}</span>
              <CopyButton text={txid || ""} />
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
            <dt className="text-gray-400 text-sm">Size / Virtual Size</dt>
            <dd className="text-gray-100 font-mono mt-1">
              {txSize.size} / {txSize.vsize} bytes
            </dd>
          </div>
          <div>
            <dt className="text-gray-400 text-sm">Weight</dt>
            <dd className="text-gray-100 font-mono mt-1">{txSize.weight} WU</dd>
          </div>
          <div>
            <dt className="text-gray-400 text-sm">Inputs</dt>
            <dd className="text-gray-100 mt-1">{transaction.input.length}</dd>
          </div>
          <div>
            <dt className="text-gray-400 text-sm">Outputs</dt>
            <dd className="text-gray-100 mt-1">{transaction.output.length}</dd>
          </div>
          {inputValues && (
            <div>
              <dt className="text-gray-400 text-sm">Total Input</dt>
              <dd className="text-gray-100 font-mono mt-1">
                {(inputValues.totalInput / 100_000_000).toFixed(8)} BTC
              </dd>
            </div>
          )}
          <div>
            <dt className="text-gray-400 text-sm">Total Output</dt>
            <dd className="text-gray-100 font-mono mt-1">
              {(totalOutput / 100_000_000).toFixed(8)} BTC
            </dd>
          </div>
          {inputValues && inputValues.totalInput > 0 && (
            <>
              <div>
                <dt className="text-gray-400 text-sm">Fee</dt>
                <dd className="text-gray-100 font-mono mt-1">
                  {((inputValues.totalInput - totalOutput) / 100_000_000).toFixed(8)} BTC
                </dd>
              </div>
              <div>
                <dt className="text-gray-400 text-sm">Fee Rate</dt>
                <dd className="text-gray-100 font-mono mt-1">
                  {((inputValues.totalInput - totalOutput) / txSize.vsize).toFixed(2)} sat/vB
                </dd>
              </div>
            </>
          )}
        </dl>
      </div>

      {/* Transaction Flow Visualization */}
      <TransactionSankey
        txid={txid!}
        inputs={transaction.input}
        outputs={transaction.output}
        inputValues={inputValues?.values}
        isCoinbase={isCoinbase}
      />

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
                    <Link
                      to={`/tx/${input.previous_output.txid}`}
                      className="text-blue-400 hover:text-blue-300"
                    >
                      {input.previous_output.txid}
                    </Link>
                    :{input.previous_output.vout}
                  </div>
                </>
              )}
              <div className="mt-2">
                <div className="text-gray-400 text-sm">Script Sig</div>
                <div className="font-mono text-xs text-gray-500 break-all mt-1">
                  {input.script_sig || "(empty)"}
                </div>
              </div>
              {input.witness && input.witness.length > 0 && (
                <details className="mt-2">
                  <summary className="text-gray-400 text-sm cursor-pointer hover:text-gray-300">
                    Witness ({input.witness.length} items)
                  </summary>
                  <div className="mt-1 pl-2 space-y-1">
                    {input.witness.map((w, wIdx) => (
                      <div key={wIdx} className="font-mono text-xs text-gray-500 break-all">
                        <span className="text-gray-600">[{wIdx}]</span> {w || "(empty)"}
                      </div>
                    ))}
                  </div>
                </details>
              )}
              <div className="mt-2 flex items-center gap-2 text-gray-500 text-sm">
                <span>Sequence: {input.sequence.toString(16).padStart(8, '0')}</span>
                {input.sequence < RBF_MAX_SEQUENCE && (
                  <span className="text-yellow-400 text-xs">(RBF)</span>
                )}
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
          {transaction.output.map((output, index) => {
            const scriptInfo = detectScriptType(output.script_pubkey);
            const isOpReturn = scriptInfo.type === "OP_RETURN";

            return (
              <div key={index} className="px-4 py-3">
                <div className="flex items-center justify-between mb-2">
                  <div className="flex items-center gap-2">
                    <span className="text-gray-500 text-sm">#{index}</span>
                    <ScriptTypeBadge scriptPubkey={output.script_pubkey} />
                  </div>
                  <span className={`font-mono ${isOpReturn ? "text-gray-500" : "text-bitcoin-orange"}`}>
                    {(output.value / 100_000_000).toFixed(8)} BTC
                  </span>
                </div>

                {isOpReturn ? (
                  <div className="text-gray-400 text-sm">
                    <span className="text-gray-500">Unspendable data output</span>
                    <OpReturnData scriptPubkey={output.script_pubkey} />
                  </div>
                ) : (
                  <>
                    <div className="text-gray-400 text-sm mb-1">Address</div>
                    <div className="mb-2">
                      <OutputAddress scriptPubkey={output.script_pubkey} />
                    </div>
                  </>
                )}

                <details className="text-gray-500 mt-2">
                  <summary className="text-gray-400 text-sm cursor-pointer hover:text-gray-300">
                    Script PubKey
                  </summary>
                  <div className="font-mono text-xs break-all mt-1 pl-2">
                    {output.script_pubkey}
                  </div>
                </details>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
