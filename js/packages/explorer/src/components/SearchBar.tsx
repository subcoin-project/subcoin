import { useState, FormEvent } from "react";
import { useNavigate } from "react-router-dom";
import { blockchainApi } from "@subcoin/shared";

export function SearchBar() {
  const [query, setQuery] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [searching, setSearching] = useState(false);
  const navigate = useNavigate();

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    setError(null);

    const trimmedQuery = query.trim();
    if (!trimmedQuery) return;

    setSearching(true);

    try {
      // Check if it's a block height (positive integer)
      if (isBlockHeight(trimmedQuery)) {
        navigate(`/block/${trimmedQuery}`);
        setQuery("");
        return;
      }

      // Check if it's a 64-character hex string (could be block hash or txid)
      if (isHex64(trimmedQuery)) {
        // Try to fetch as a block first
        try {
          const block = await blockchainApi.getBlock(trimmedQuery);
          if (block) {
            navigate(`/block/${trimmedQuery}`);
            setQuery("");
            return;
          }
        } catch {
          // Not a block, try as transaction
        }

        // Try as transaction
        try {
          const tx = await blockchainApi.getTransaction(trimmedQuery);
          if (tx) {
            navigate(`/tx/${trimmedQuery}`);
            setQuery("");
            return;
          }
        } catch {
          // Not a transaction either
        }

        setError("No block or transaction found with this hash.");
        return;
      }

      setError("Invalid input. Enter a block height, block hash, or transaction ID.");
    } finally {
      setSearching(false);
    }
  };

  return (
    <form onSubmit={handleSubmit} className="relative">
      <div className="flex">
        <input
          type="text"
          value={query}
          onChange={(e) => {
            setQuery(e.target.value);
            setError(null);
          }}
          placeholder="Search block, hash, or txid..."
          className="w-full sm:w-64 lg:w-96 px-3 py-2 bg-gray-800 border border-gray-700 rounded-l-md text-gray-200 text-sm placeholder-gray-500 focus:outline-none focus:border-bitcoin-orange"
          disabled={searching}
        />
        <button
          type="submit"
          disabled={searching}
          className="px-4 py-2 bg-bitcoin-orange text-white rounded-r-md hover:bg-orange-600 transition-colors disabled:opacity-50"
        >
          {searching ? <LoadingSpinner /> : <SearchIcon />}
        </button>
      </div>
      {error && (
        <div className="absolute top-full left-0 mt-1 px-3 py-1 bg-red-900/80 text-red-200 text-xs rounded">
          {error}
        </div>
      )}
    </form>
  );
}

function SearchIcon() {
  return (
    <svg
      className="w-4 h-4"
      fill="none"
      stroke="currentColor"
      viewBox="0 0 24 24"
    >
      <path
        strokeLinecap="round"
        strokeLinejoin="round"
        strokeWidth={2}
        d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z"
      />
    </svg>
  );
}

function LoadingSpinner() {
  return (
    <svg
      className="w-4 h-4 animate-spin"
      fill="none"
      viewBox="0 0 24 24"
    >
      <circle
        className="opacity-25"
        cx="12"
        cy="12"
        r="10"
        stroke="currentColor"
        strokeWidth="4"
      />
      <path
        className="opacity-75"
        fill="currentColor"
        d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"
      />
    </svg>
  );
}

/**
 * Check if the input is a valid block height (positive integer)
 */
function isBlockHeight(input: string): boolean {
  const num = parseInt(input, 10);
  return !isNaN(num) && num >= 0 && num.toString() === input;
}

/**
 * Check if the input looks like a 64-character hex string (block hash or txid)
 */
function isHex64(input: string): boolean {
  return /^[0-9a-fA-F]{64}$/.test(input);
}
