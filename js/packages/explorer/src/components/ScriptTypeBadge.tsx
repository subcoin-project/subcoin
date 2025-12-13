import { detectScriptType, decodeOpReturnData, type ScriptInfo } from "@subcoin/shared";

interface ScriptTypeBadgeProps {
  scriptPubkey: string;
  showDescription?: boolean;
}

/**
 * Displays a badge showing the script type (P2PKH, P2SH, P2WPKH, etc.)
 */
export function ScriptTypeBadge({ scriptPubkey, showDescription = false }: ScriptTypeBadgeProps) {
  const scriptInfo = detectScriptType(scriptPubkey);

  return (
    <span
      className={`inline-flex items-center px-2 py-0.5 rounded text-xs font-medium ${scriptInfo.colorClass}`}
      title={scriptInfo.description}
    >
      {scriptInfo.label}
      {showDescription && (
        <span className="ml-1 opacity-75">- {scriptInfo.description}</span>
      )}
    </span>
  );
}

interface OpReturnDataProps {
  scriptPubkey: string;
}

/**
 * Displays decoded OP_RETURN data if it's valid UTF-8 text
 */
export function OpReturnData({ scriptPubkey }: OpReturnDataProps) {
  const decodedText = decodeOpReturnData(scriptPubkey);

  if (!decodedText) {
    return null;
  }

  return (
    <div className="mt-1 text-sm">
      <span className="text-gray-400">Data: </span>
      <span className="text-gray-300 font-mono">{decodedText}</span>
    </div>
  );
}

/**
 * Full output display with script type badge and optional OP_RETURN data
 */
export function ScriptTypeInfo({ scriptPubkey }: { scriptPubkey: string }) {
  const scriptInfo = detectScriptType(scriptPubkey);

  return (
    <div className="flex flex-col gap-1">
      <ScriptTypeBadge scriptPubkey={scriptPubkey} />
      {scriptInfo.type === "OP_RETURN" && (
        <OpReturnData scriptPubkey={scriptPubkey} />
      )}
    </div>
  );
}
