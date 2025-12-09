import { useState, useRef, useEffect } from "react";
import { useConnection } from "../contexts/ConnectionContext";

export function EndpointSettings() {
  const { endpoint, setEndpoint, isConnected } = useConnection();
  const [isEditing, setIsEditing] = useState(false);
  const [inputValue, setInputValue] = useState(endpoint);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (isEditing && inputRef.current) {
      inputRef.current.focus();
      inputRef.current.select();
    }
  }, [isEditing]);

  const handleSubmit = () => {
    const trimmed = inputValue.trim();
    if (trimmed && trimmed !== endpoint) {
      setEndpoint(trimmed);
    }
    setIsEditing(false);
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter") {
      handleSubmit();
    } else if (e.key === "Escape") {
      setInputValue(endpoint);
      setIsEditing(false);
    }
  };

  const handleBlur = () => {
    // Small delay to allow click events to process
    setTimeout(() => {
      if (isEditing) {
        handleSubmit();
      }
    }, 100);
  };

  if (isEditing) {
    return (
      <div className="flex items-center gap-1">
        <input
          ref={inputRef}
          type="text"
          value={inputValue}
          onChange={(e) => setInputValue(e.target.value)}
          onKeyDown={handleKeyDown}
          onBlur={handleBlur}
          className="bg-gray-800 border border-gray-600 rounded px-2 py-0.5 text-sm text-gray-300 w-40 focus:outline-none focus:border-bitcoin-orange"
          placeholder="host:port"
        />
      </div>
    );
  }

  return (
    <button
      onClick={() => {
        setInputValue(endpoint);
        setIsEditing(true);
      }}
      className="flex items-center gap-1.5 px-2 py-1 rounded border border-gray-700 bg-gray-800/50 hover:border-gray-600 hover:bg-gray-800 transition-colors group"
      title="Click to change RPC endpoint"
    >
      {/* Connection status indicator */}
      <span
        className={`w-2 h-2 rounded-full ${isConnected ? "bg-green-500" : "bg-red-500"}`}
        title={isConnected ? "Connected" : "Disconnected"}
      />
      <span className="text-gray-400 text-sm">
        {endpoint}
      </span>
      <svg
        className="w-3 h-3 text-gray-500 group-hover:text-gray-300 transition-colors"
        fill="none"
        stroke="currentColor"
        viewBox="0 0 24 24"
      >
        <path
          strokeLinecap="round"
          strokeLinejoin="round"
          strokeWidth={2}
          d="M15.232 5.232l3.536 3.536m-2.036-5.036a2.5 2.5 0 113.536 3.536L6.5 21.036H3v-3.572L16.732 3.732z"
        />
      </svg>
    </button>
  );
}
