/**
 * Blockchain API - wraps blockchain_* RPC methods
 */

import type { Block, BlockHash, BlockHeader, BlockWithTxids, Transaction, TxIn, Txid } from "../types/block";
import { getDefaultClient, SubcoinRpcClient } from "./client";

/** Raw transaction input as returned by RPC (previous_output is a string "txid:vout") */
interface RawTxIn {
  previous_output: string;
  script_sig: string;
  sequence: number;
  witness?: string[];
}

/** Raw transaction as returned by RPC */
interface RawTransaction {
  version: number;
  lock_time: number;
  input: RawTxIn[];
  output: { value: number; script_pubkey: string }[];
}

/** Parse a "txid:vout" string into {txid, vout} object */
function parsePreviousOutput(outpoint: string): { txid: string; vout: number } {
  const lastColon = outpoint.lastIndexOf(":");
  if (lastColon === -1) {
    return { txid: outpoint, vout: 0 };
  }
  return {
    txid: outpoint.slice(0, lastColon),
    vout: parseInt(outpoint.slice(lastColon + 1), 10),
  };
}

/** Transform raw RPC transaction to typed Transaction */
function transformTransaction(raw: RawTransaction): Transaction {
  return {
    version: raw.version,
    lock_time: raw.lock_time,
    input: raw.input.map((inp): TxIn => ({
      previous_output: parsePreviousOutput(inp.previous_output),
      script_sig: inp.script_sig,
      sequence: inp.sequence,
      witness: inp.witness,
    })),
    output: raw.output,
  };
}

/** Raw block as returned by RPC */
interface RawBlock {
  header: BlockHeader;
  txdata: RawTransaction[];
}

/** Raw block with txids as returned by RPC */
interface RawBlockWithTxids {
  header: BlockHeader;
  txids: string[];
  txdata: RawTransaction[];
}

/** Transform raw RPC block to typed Block */
function transformBlock(raw: RawBlock): Block {
  return {
    header: raw.header,
    txdata: raw.txdata.map(transformTransaction),
  };
}

/** Transform raw RPC block with txids to typed BlockWithTxids */
function transformBlockWithTxids(raw: RawBlockWithTxids): BlockWithTxids {
  return {
    header: raw.header,
    txids: raw.txids,
    txdata: raw.txdata.map(transformTransaction),
  };
}

export class BlockchainApi {
  constructor(private client: SubcoinRpcClient = getDefaultClient()) {}

  /**
   * Get block header by hash (or best block if not specified)
   */
  async getHeader(hash?: BlockHash): Promise<BlockHeader | null> {
    return this.client.request<BlockHeader | null>("blockchain_getHeader", hash ? [hash] : []);
  }

  /**
   * Get full block by hash (or best block if not specified)
   */
  async getBlock(hash?: BlockHash): Promise<Block | null> {
    const raw = await this.client.request<RawBlock | null>("blockchain_getBlock", hash ? [hash] : []);
    return raw ? transformBlock(raw) : null;
  }

  /**
   * Get full block by height
   */
  async getBlockByNumber(height?: number): Promise<Block | null> {
    const raw = await this.client.request<RawBlock | null>(
      "blockchain_getBlockByNumber",
      height !== undefined ? [height] : []
    );
    return raw ? transformBlock(raw) : null;
  }

  /**
   * Get full block with transaction IDs by hash
   */
  async getBlockWithTxids(hash?: BlockHash): Promise<BlockWithTxids | null> {
    const raw = await this.client.request<RawBlockWithTxids | null>(
      "blockchain_getBlockWithTxids",
      hash ? [hash] : []
    );
    return raw ? transformBlockWithTxids(raw) : null;
  }

  /**
   * Get full block with transaction IDs by height
   */
  async getBlockWithTxidsByNumber(height?: number): Promise<BlockWithTxids | null> {
    const raw = await this.client.request<RawBlockWithTxids | null>(
      "blockchain_getBlockWithTxidsByNumber",
      height !== undefined ? [height] : []
    );
    return raw ? transformBlockWithTxids(raw) : null;
  }

  /**
   * Get raw block in hex format by hash
   */
  async getRawBlock(hash?: BlockHash): Promise<string | null> {
    return this.client.request<string | null>("blockchain_getRawBlock", hash ? [hash] : []);
  }

  /**
   * Get raw block in hex format by height
   */
  async getRawBlockByNumber(height?: number): Promise<string | null> {
    return this.client.request<string | null>(
      "blockchain_getRawBlockByNumber",
      height !== undefined ? [height] : []
    );
  }

  /**
   * Get transaction by txid
   */
  async getTransaction(txid: Txid): Promise<Transaction | null> {
    const raw = await this.client.request<RawTransaction | null>("blockchain_getTransaction", [txid]);
    return raw ? transformTransaction(raw) : null;
  }

  /**
   * Get current best block hash
   */
  async getBestBlockHash(): Promise<BlockHash> {
    return this.client.request<BlockHash>("blockchain_getBestBlockHash", []);
  }

  /**
   * Get block hash by height
   */
  async getBlockHash(height: number): Promise<BlockHash | null> {
    return this.client.request<BlockHash | null>("blockchain_getBlockHash", [height]);
  }

  /**
   * Get block height by hash
   */
  async getBlockNumber(hash: BlockHash): Promise<number | null> {
    return this.client.request<number | null>("blockchain_getBlockNumber", [hash]);
  }

  /**
   * Decode a script_pubkey (hex) to a Bitcoin address.
   * Returns null if the script cannot be converted to an address
   * (e.g., OP_RETURN outputs, non-standard scripts).
   */
  async decodeScriptPubkey(scriptPubkeyHex: string): Promise<string | null> {
    return this.client.request<string | null>("blockchain_decodeScriptPubkey", [scriptPubkeyHex]);
  }
}

/**
 * Default blockchain API instance
 */
export const blockchainApi = new BlockchainApi();
