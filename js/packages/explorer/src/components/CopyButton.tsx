import { useState, useCallback } from "react";

interface CopyButtonProps {
  text: string;
  className?: string;
}

export function CopyButton({ text, className = "" }: CopyButtonProps) {
  const [copied, setCopied] = useState(false);

  const handleCopy = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch (err) {
      console.error("Failed to copy:", err);
    }
  }, [text]);

  return (
    <button
      onClick={handleCopy}
      className={`p-1 text-gray-500 hover:text-gray-300 transition-colors ${className}`}
      title={copied ? "Copied!" : "Copy to clipboard"}
    >
      {copied ? (
        <CheckIcon className="w-4 h-4 text-green-500" />
      ) : (
        <CopyIcon className="w-4 h-4" />
      )}
    </button>
  );
}

function CopyIcon({ className }: { className?: string }) {
  return (
    <svg
      className={className}
      fill="none"
      stroke="currentColor"
      viewBox="0 0 24 24"
    >
      <path
        strokeLinecap="round"
        strokeLinejoin="round"
        strokeWidth={2}
        d="M8 16H6a2 2 0 01-2-2V6a2 2 0 012-2h8a2 2 0 012 2v2m-6 12h8a2 2 0 002-2v-8a2 2 0 00-2-2h-8a2 2 0 00-2 2v8a2 2 0 002 2z"
      />
    </svg>
  );
}

function CheckIcon({ className }: { className?: string }) {
  return (
    <svg
      className={className}
      fill="none"
      stroke="currentColor"
      viewBox="0 0 24 24"
    >
      <path
        strokeLinecap="round"
        strokeLinejoin="round"
        strokeWidth={2}
        d="M5 13l4 4L19 7"
      />
    </svg>
  );
}

/**
 * A wrapper component that displays text with a copy button
 */
interface CopyableTextProps {
  text: string;
  truncate?: boolean;
  className?: string;
  mono?: boolean;
}

export function CopyableText({ text, truncate, className = "", mono }: CopyableTextProps) {
  const displayText = truncate && text.length > 16
    ? `${text.slice(0, 8)}...${text.slice(-8)}`
    : text;

  return (
    <span className={`inline-flex items-center gap-1 ${className}`}>
      <span className={`${mono ? "font-mono" : ""} break-all`}>{displayText}</span>
      <CopyButton text={text} />
    </span>
  );
}
