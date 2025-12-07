import { useEffect, useState } from "react";
import { useParams, Link } from "react-router-dom";
import { blockchainApi, systemApi } from "@subcoin/shared";
import type { BlockWithTxids, Transaction } from "@subcoin/shared";
import { BlockDetailSkeleton } from "../components/Skeleton";
import { CopyButton } from "../components/CopyButton";

export function BlockDetail() {
  const { hashOrHeight } = useParams<{ hashOrHeight: string }>();
  const [block, setBlock] = useState<BlockWithTxids | null>(null);
  const [blockHash, setBlockHash] = useState<string>("");
  const [blockHeight, setBlockHeight] = useState<number | null>(null);
  const [currentHeight, setCurrentHeight] = useState<number | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    async function fetchBlock() {
      if (!hashOrHeight) return;

      try {
        setLoading(true);
        setError(null);

        // Determine if it's a height or hash
        const isHeight = /^\d+$/.test(hashOrHeight);

        // Fetch current height for confirmations
        const syncState = await systemApi.syncState();
        setCurrentHeight(syncState.currentBlock);

        if (isHeight) {
          const height = parseInt(hashOrHeight, 10);
          const fetchedBlock = await blockchainApi.getBlockWithTxidsByNumber(height);
          const hash = await blockchainApi.getBlockHash(height);
          setBlock(fetchedBlock);
          setBlockHash(hash || "");
          setBlockHeight(height);
        } else {
          const fetchedBlock = await blockchainApi.getBlockWithTxids(hashOrHeight);
          const height = await blockchainApi.getBlockNumber(hashOrHeight);
          setBlock(fetchedBlock);
          setBlockHash(hashOrHeight);
          setBlockHeight(height);
        }
      } catch (err) {
        setError(err instanceof Error ? err.message : "Failed to fetch block");
      } finally {
        setLoading(false);
      }
    }

    fetchBlock();
  }, [hashOrHeight]);

  if (loading) {
    return <BlockDetailSkeleton />;
  }

  if (error || !block) {
    return (
      <div className="bg-red-900/20 border border-red-800 rounded-lg p-4">
        <h3 className="text-red-400 font-medium">Error</h3>
        <p className="text-red-300 text-sm mt-1">{error || "Block not found"}</p>
      </div>
    );
  }

  const confirmations = blockHeight !== null && currentHeight !== null
    ? currentHeight - blockHeight + 1
    : null;

  return (
    <div className="space-y-6">
      {/* Block Header */}
      <div className="bg-bitcoin-dark rounded-lg border border-gray-800 p-4">
        <div className="flex items-center justify-between mb-4">
          <div className="flex items-center gap-3">
            <h1 className="text-xl font-bold text-gray-100">
              Block #{blockHeight?.toLocaleString()}
            </h1>
            {confirmations !== null && (
              <span className={`text-xs px-2 py-1 rounded ${
                confirmations >= 6
                  ? "bg-green-900/30 text-green-400"
                  : confirmations >= 1
                  ? "bg-yellow-900/30 text-yellow-400"
                  : "bg-gray-700 text-gray-400"
              }`}>
                {confirmations.toLocaleString()} confirmation{confirmations !== 1 ? "s" : ""}
              </span>
            )}
          </div>
          <div className="flex space-x-2">
            {blockHeight !== null && blockHeight > 0 && (
              <Link
                to={`/block/${blockHeight - 1}`}
                className="bg-gray-700 hover:bg-gray-600 text-gray-300 px-3 py-1 rounded text-sm"
              >
                Previous
              </Link>
            )}
            <Link
              to={`/block/${(blockHeight ?? 0) + 1}`}
              className="bg-gray-700 hover:bg-gray-600 text-gray-300 px-3 py-1 rounded text-sm"
            >
              Next
            </Link>
          </div>
        </div>

        <dl className="grid grid-cols-1 md:grid-cols-2 gap-4">
          <InfoRow label="Hash" value={blockHash} mono copyable />
          <InfoRow
            label="Timestamp"
            value={new Date(block.header.time * 1000).toLocaleString()}
          />
          <InfoRow label="Version" value={`0x${block.header.version.toString(16)}`} />
          <InfoRow label="Nonce" value={block.header.nonce.toLocaleString()} />
          <InfoRow label="Bits" value={`0x${block.header.bits.toString(16)}`} />
          <InfoRow label="Transactions" value={block.txdata.length.toString()} />
          <InfoRow label="Merkle Root" value={block.header.merkle_root} mono fullWidth copyable />
          <InfoRow label="Previous Block" value={block.header.prev_blockhash} mono fullWidth link={`/block/${block.header.prev_blockhash}`} copyable />
        </dl>
      </div>

      {/* Transactions */}
      <div className="bg-bitcoin-dark rounded-lg border border-gray-800">
        <div className="px-4 py-3 border-b border-gray-800">
          <h2 className="text-lg font-medium text-gray-100">
            Transactions ({block.txdata.length})
          </h2>
        </div>
        <div className="divide-y divide-gray-800">
          {block.txdata.map((tx, index) => (
            <TransactionRow key={block.txids[index]} tx={tx} txid={block.txids[index]} index={index} />
          ))}
        </div>
      </div>
    </div>
  );
}

function InfoRow({
  label,
  value,
  mono,
  fullWidth,
  link,
  copyable,
}: {
  label: string;
  value: string;
  mono?: boolean;
  fullWidth?: boolean;
  link?: string;
  copyable?: boolean;
}) {
  const content = link ? (
    <Link to={link} className="text-bitcoin-orange hover:underline">
      {value}
    </Link>
  ) : (
    value
  );

  return (
    <div className={fullWidth ? "md:col-span-2" : ""}>
      <dt className="text-gray-400 text-sm">{label}</dt>
      <dd
        className={`text-gray-100 mt-1 break-all flex items-start gap-1 ${mono ? "font-mono text-sm" : ""}`}
      >
        <span className="flex-1">{content}</span>
        {copyable && <CopyButton text={value} />}
      </dd>
    </div>
  );
}

function TransactionRow({ tx, txid, index }: { tx: Transaction; txid: string; index: number }) {
  const [expanded, setExpanded] = useState(false);

  const isCoinbase = tx.input.length === 1 && tx.input[0].previous_output.txid === "0000000000000000000000000000000000000000000000000000000000000000";

  const totalOutput = tx.output.reduce((sum, out) => sum + out.value, 0);

  return (
    <div className="px-4 py-3">
      <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-2">
        <div className="flex items-center space-x-3 min-w-0">
          <span className="text-gray-500 text-sm flex-shrink-0">#{index}</span>
          {isCoinbase && (
            <span className="bg-bitcoin-orange/20 text-bitcoin-orange text-xs px-2 py-0.5 rounded flex-shrink-0">
              Coinbase
            </span>
          )}
          <Link
            to={`/tx/${txid}`}
            className="font-mono text-sm text-bitcoin-orange hover:underline truncate"
            onClick={(e) => e.stopPropagation()}
          >
            {txid.slice(0, 16)}...{txid.slice(-8)}
          </Link>
          <CopyButton text={txid} />
        </div>
        <div
          className="flex items-center space-x-4 cursor-pointer flex-shrink-0"
          onClick={() => setExpanded(!expanded)}
        >
          <span className="text-gray-300 text-sm">
            {tx.input.length} input{tx.input.length !== 1 ? "s" : ""} →{" "}
            {tx.output.length} output{tx.output.length !== 1 ? "s" : ""}
          </span>
          <span className="text-gray-100 font-mono text-sm">
            {(totalOutput / 100_000_000).toFixed(8)} BTC
          </span>
          <span className="text-gray-500">{expanded ? "▼" : "▶"}</span>
        </div>
      </div>

      {expanded && (
        <div className="mt-4 grid grid-cols-2 gap-4">
          {/* Inputs */}
          <div>
            <h4 className="text-gray-400 text-sm mb-2">Inputs</h4>
            <div className="space-y-2">
              {tx.input.map((input, i) => (
                <div key={i} className="bg-gray-800/50 rounded p-2 text-xs">
                  {isCoinbase ? (
                    <span className="text-bitcoin-orange">Coinbase</span>
                  ) : (
                    <>
                      <div className="font-mono text-gray-300 break-all">
                        {input.previous_output.txid.slice(0, 16)}...
                      </div>
                      <div className="text-gray-500">
                        Output #{input.previous_output.vout}
                      </div>
                    </>
                  )}
                </div>
              ))}
            </div>
          </div>

          {/* Outputs */}
          <div>
            <h4 className="text-gray-400 text-sm mb-2">Outputs</h4>
            <div className="space-y-2">
              {tx.output.map((output, i) => (
                <div key={i} className="bg-gray-800/50 rounded p-2 text-xs">
                  <div className="text-gray-100 font-mono">
                    {(output.value / 100_000_000).toFixed(8)} BTC
                  </div>
                  <div className="font-mono text-gray-500 break-all">
                    {output.script_pubkey.slice(0, 32)}...
                  </div>
                </div>
              ))}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
