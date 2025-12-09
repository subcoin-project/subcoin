/**
 * Bitcoin script type detection utilities
 */

export type ScriptType =
  | "P2PKH"      // Pay to Public Key Hash (legacy, starts with 1)
  | "P2SH"       // Pay to Script Hash (starts with 3)
  | "P2WPKH"     // Pay to Witness Public Key Hash (native segwit, bc1q)
  | "P2WSH"      // Pay to Witness Script Hash (native segwit, bc1q)
  | "P2TR"       // Pay to Taproot (bc1p)
  | "P2PK"       // Pay to Public Key (legacy, rare)
  | "OP_RETURN"  // Data output (unspendable)
  | "P2MS"       // Pay to Multisig (bare multisig, rare)
  | "Unknown";

export interface ScriptInfo {
  type: ScriptType;
  /** Human-readable description */
  description: string;
  /** Short label for display */
  label: string;
  /** CSS color class */
  colorClass: string;
}

/**
 * Detect the script type from a script_pubkey hex string
 */
export function detectScriptType(scriptPubkeyHex: string): ScriptInfo {
  const script = scriptPubkeyHex.toLowerCase();

  // P2PKH: OP_DUP OP_HASH160 <20 bytes> OP_EQUALVERIFY OP_CHECKSIG
  // Pattern: 76a914{40 hex chars}88ac
  if (/^76a914[0-9a-f]{40}88ac$/.test(script)) {
    return {
      type: "P2PKH",
      description: "Pay to Public Key Hash",
      label: "P2PKH",
      colorClass: "bg-blue-500/20 text-blue-400",
    };
  }

  // P2SH: OP_HASH160 <20 bytes> OP_EQUAL
  // Pattern: a914{40 hex chars}87
  if (/^a914[0-9a-f]{40}87$/.test(script)) {
    return {
      type: "P2SH",
      description: "Pay to Script Hash",
      label: "P2SH",
      colorClass: "bg-purple-500/20 text-purple-400",
    };
  }

  // P2WPKH: OP_0 <20 bytes>
  // Pattern: 0014{40 hex chars}
  if (/^0014[0-9a-f]{40}$/.test(script)) {
    return {
      type: "P2WPKH",
      description: "Pay to Witness Public Key Hash (SegWit)",
      label: "P2WPKH",
      colorClass: "bg-green-500/20 text-green-400",
    };
  }

  // P2WSH: OP_0 <32 bytes>
  // Pattern: 0020{64 hex chars}
  if (/^0020[0-9a-f]{64}$/.test(script)) {
    return {
      type: "P2WSH",
      description: "Pay to Witness Script Hash (SegWit)",
      label: "P2WSH",
      colorClass: "bg-teal-500/20 text-teal-400",
    };
  }

  // P2TR: OP_1 <32 bytes>
  // Pattern: 5120{64 hex chars}
  if (/^5120[0-9a-f]{64}$/.test(script)) {
    return {
      type: "P2TR",
      description: "Pay to Taproot",
      label: "P2TR",
      colorClass: "bg-orange-500/20 text-orange-400",
    };
  }

  // OP_RETURN: OP_RETURN <data>
  // Pattern: 6a...
  if (script.startsWith("6a")) {
    return {
      type: "OP_RETURN",
      description: "Data Output (Unspendable)",
      label: "OP_RETURN",
      colorClass: "bg-gray-500/20 text-gray-400",
    };
  }

  // P2PK: <33 or 65 bytes pubkey> OP_CHECKSIG
  // Pattern: {66 or 130 hex chars}ac
  if (/^([0-9a-f]{66}|[0-9a-f]{130})ac$/.test(script)) {
    return {
      type: "P2PK",
      description: "Pay to Public Key (Legacy)",
      label: "P2PK",
      colorClass: "bg-yellow-500/20 text-yellow-400",
    };
  }

  // Bare multisig (P2MS) - starts with OP_1-OP_16, ends with OP_CHECKMULTISIG
  // Pattern: 5{1-9|a-f}...ae
  if (/^5[1-9a-f].*ae$/.test(script)) {
    return {
      type: "P2MS",
      description: "Pay to Multisig (Bare)",
      label: "P2MS",
      colorClass: "bg-pink-500/20 text-pink-400",
    };
  }

  return {
    type: "Unknown",
    description: "Non-standard Script",
    label: "Unknown",
    colorClass: "bg-red-500/20 text-red-400",
  };
}

/**
 * Try to decode OP_RETURN data as UTF-8 text
 */
export function decodeOpReturnData(scriptPubkeyHex: string): string | null {
  if (!scriptPubkeyHex.toLowerCase().startsWith("6a")) {
    return null;
  }

  try {
    // Skip OP_RETURN (6a) and try to decode the rest
    // The next byte(s) indicate the push length
    const dataHex = scriptPubkeyHex.slice(2);

    // Simple case: single push
    let hexData = dataHex;

    // Check for push opcodes
    const firstByte = parseInt(dataHex.slice(0, 2), 16);
    if (firstByte <= 0x4b) {
      // Direct push: first byte is length
      hexData = dataHex.slice(2, 2 + firstByte * 2);
    } else if (firstByte === 0x4c) {
      // OP_PUSHDATA1: next byte is length
      const len = parseInt(dataHex.slice(2, 4), 16);
      hexData = dataHex.slice(4, 4 + len * 2);
    } else if (firstByte === 0x4d) {
      // OP_PUSHDATA2: next 2 bytes are length (little endian)
      const len = parseInt(dataHex.slice(4, 6) + dataHex.slice(2, 4), 16);
      hexData = dataHex.slice(6, 6 + len * 2);
    }

    // Convert hex to string
    const bytes = hexData.match(/.{2}/g);
    if (!bytes) return null;

    const text = bytes
      .map((byte) => String.fromCharCode(parseInt(byte, 16)))
      .join("");

    // Check if it's printable ASCII
    if (/^[\x20-\x7e]+$/.test(text)) {
      return text;
    }

    return null;
  } catch {
    return null;
  }
}
