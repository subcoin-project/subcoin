import { useEffect, useState, useMemo } from "react";
import { useParams, Link, useSearchParams } from "react-router-dom";
import { blockchainApi, systemApi } from "@subcoin/shared";
import type { BlockWithTxids, Transaction } from "@subcoin/shared";
import { BlockDetailSkeleton } from "../components/Skeleton";
import { CopyButton } from "../components/CopyButton";

const TXS_PER_PAGE = 25;

// Block header is always 80 bytes
const BLOCK_HEADER_SIZE = 80;

/**
 * Estimate transaction size from transaction data.
 * This is an approximation since we don't have the raw serialized data.
 */
function estimateTxSize(tx: Transaction): { size: number; weight: number; hasWitness: boolean } {
  // Check if any input has witness data
  const hasWitness = tx.input.some(inp => inp.witness && inp.witness.length > 0);

  // Base transaction overhead: version (4) + locktime (4) + input count (1-3) + output count (1-3)
  let baseSize = 4 + 4 + varIntSize(tx.input.length) + varIntSize(tx.output.length);

  // Inputs: each input has outpoint (36) + scriptSig length (1-3) + scriptSig + sequence (4)
  for (const inp of tx.input) {
    const scriptSigLen = inp.script_sig.length / 2; // hex string to bytes
    baseSize += 36 + varIntSize(scriptSigLen) + scriptSigLen + 4;
  }

  // Outputs: each output has value (8) + scriptPubKey length (1-3) + scriptPubKey
  for (const out of tx.output) {
    const scriptPubKeyLen = out.script_pubkey.length / 2;
    baseSize += 8 + varIntSize(scriptPubKeyLen) + scriptPubKeyLen;
  }

  let witnessSize = 0;
  if (hasWitness) {
    // Marker (1) + flag (1) for SegWit
    baseSize += 2;

    // Witness data for each input
    for (const inp of tx.input) {
      if (inp.witness && inp.witness.length > 0) {
        witnessSize += varIntSize(inp.witness.length);
        for (const w of inp.witness) {
          const wLen = w.length / 2;
          witnessSize += varIntSize(wLen) + wLen;
        }
      } else {
        witnessSize += 1; // empty witness stack
      }
    }
  }

  const totalSize = baseSize + witnessSize;
  // Weight = base_size * 4 + witness_size (BIP 141)
  // For non-witness tx: weight = size * 4
  const weight = hasWitness
    ? (baseSize - 2) * 4 + 2 + witnessSize  // -2 for marker/flag which count as witness
    : baseSize * 4;

  return { size: totalSize, weight, hasWitness };
}

/**
 * Calculate variable integer size (Bitcoin's CompactSize)
 */
function varIntSize(n: number): number {
  if (n < 0xfd) return 1;
  if (n <= 0xffff) return 3;
  if (n <= 0xffffffff) return 5;
  return 9;
}

/**
 * Format bytes to human readable string
 */
function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(2)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(2)} MB`;
}

/**
 * Format weight units
 */
function formatWeight(wu: number): string {
  if (wu < 1000) return `${wu} WU`;
  if (wu < 1000000) return `${(wu / 1000).toFixed(2)} kWU`;
  return `${(wu / 1000000).toFixed(2)} MWU`;
}

export function BlockDetail() {
  const { hashOrHeight } = useParams<{ hashOrHeight: string }>();
  const [searchParams, setSearchParams] = useSearchParams();
  const [block, setBlock] = useState<BlockWithTxids | null>(null);
  const [blockHash, setBlockHash] = useState<string>("");
  const [blockHeight, setBlockHeight] = useState<number | null>(null);
  const [currentHeight, setCurrentHeight] = useState<number | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  // Get page from URL params, default to 1
  const page = parseInt(searchParams.get("page") || "1", 10);

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

  // Calculate block size and weight (must be before conditional returns)
  const blockStats = useMemo(() => {
    if (!block) return null;

    let totalSize = BLOCK_HEADER_SIZE + varIntSize(block.txdata.length);
    let totalWeight = BLOCK_HEADER_SIZE * 4 + varIntSize(block.txdata.length) * 4;
    let witnessCount = 0;

    for (const tx of block.txdata) {
      const txStats = estimateTxSize(tx);
      totalSize += txStats.size;
      totalWeight += txStats.weight;
      if (txStats.hasWitness) witnessCount++;
    }

    // Virtual size (vsize) = (weight + 3) / 4
    const vsize = Math.ceil(totalWeight / 4);

    return {
      size: totalSize,
      weight: totalWeight,
      vsize,
      witnessCount,
      hasSegWit: witnessCount > 0,
    };
  }, [block]);

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

  // Pagination calculations
  const totalTxs = block.txdata.length;
  const totalPages = Math.ceil(totalTxs / TXS_PER_PAGE);
  const startIndex = (page - 1) * TXS_PER_PAGE;
  const endIndex = Math.min(startIndex + TXS_PER_PAGE, totalTxs);
  const paginatedTxs = block.txdata.slice(startIndex, endIndex);
  const paginatedTxids = block.txids.slice(startIndex, endIndex);

  const goToPage = (newPage: number) => {
    if (newPage >= 1 && newPage <= totalPages) {
      setSearchParams({ page: newPage.toString() });
      // Scroll to transactions section
      document.getElementById("transactions")?.scrollIntoView({ behavior: "smooth" });
    }
  };

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
          <InfoRow label="Transactions" value={block.txdata.length.toLocaleString()} />
          {blockStats && (
            <>
              <InfoRow label="Size" value={formatBytes(blockStats.size)} />
              <InfoRow label="Weight" value={formatWeight(blockStats.weight)} />
              <InfoRow label="Virtual Size" value={formatBytes(blockStats.vsize)} />
              {blockStats.hasSegWit && (
                <InfoRow
                  label="SegWit Transactions"
                  value={`${blockStats.witnessCount} of ${block.txdata.length} (${Math.round((blockStats.witnessCount / block.txdata.length) * 100)}%)`}
                />
              )}
            </>
          )}
          <InfoRow label="Merkle Root" value={block.header.merkle_root} mono fullWidth copyable />
          <InfoRow label="Previous Block" value={block.header.prev_blockhash} mono fullWidth link={`/block/${block.header.prev_blockhash}`} copyable />
        </dl>
      </div>

      {/* Transactions */}
      <div id="transactions" className="bg-bitcoin-dark rounded-lg border border-gray-800">
        <div className="px-4 py-3 border-b border-gray-800 flex items-center justify-between">
          <h2 className="text-lg font-medium text-gray-100">
            Transactions ({totalTxs.toLocaleString()})
          </h2>
          {totalPages > 1 && (
            <span className="text-gray-400 text-sm">
              Page {page}/{totalPages}
            </span>
          )}
        </div>
        <div className="divide-y divide-gray-800">
          {paginatedTxs.map((tx, localIndex) => {
            const globalIndex = startIndex + localIndex;
            return (
              <TransactionRow
                key={paginatedTxids[localIndex]}
                tx={tx}
                txid={paginatedTxids[localIndex]}
                index={globalIndex}
              />
            );
          })}
        </div>

        {/* Pagination */}
        {totalPages > 1 && (
          <div className="px-4 py-3 border-t border-gray-800 flex items-center justify-between">
            <div className="text-gray-400 text-sm">
              Showing {(startIndex + 1).toLocaleString()} to {endIndex.toLocaleString()} of {totalTxs.toLocaleString()} transactions
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

              {/* Page numbers */}
              <div className="hidden sm:flex items-center space-x-1">
                {getPageNumbers(page, totalPages).map((pageNum, idx) => (
                  pageNum === '...' ? (
                    <span key={`ellipsis-${idx}`} className="px-2 text-gray-500">...</span>
                  ) : (
                    <button
                      key={pageNum}
                      onClick={() => goToPage(pageNum as number)}
                      className={`px-3 py-1 rounded text-sm ${
                        page === pageNum
                          ? "bg-bitcoin-orange text-white"
                          : "bg-gray-700 text-gray-300 hover:bg-gray-600"
                      }`}
                    >
                      {pageNum}
                    </button>
                  )
                ))}
              </div>

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
        )}
      </div>
    </div>
  );
}

// Helper function to generate page numbers with ellipsis
function getPageNumbers(currentPage: number, totalPages: number): (number | string)[] {
  const pages: (number | string)[] = [];
  const showPages = 5; // Number of page buttons to show

  if (totalPages <= showPages + 2) {
    // Show all pages if total is small
    for (let i = 1; i <= totalPages; i++) {
      pages.push(i);
    }
  } else {
    // Always show first page
    pages.push(1);

    if (currentPage > 3) {
      pages.push('...');
    }

    // Show pages around current page
    const start = Math.max(2, currentPage - 1);
    const end = Math.min(totalPages - 1, currentPage + 1);

    for (let i = start; i <= end; i++) {
      pages.push(i);
    }

    if (currentPage < totalPages - 2) {
      pages.push('...');
    }

    // Always show last page
    pages.push(totalPages);
  }

  return pages;
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
