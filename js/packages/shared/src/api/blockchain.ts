/**
 * Blockchain API - wraps blockchain_* RPC methods
 */

import type { Block, BlockHash, BlockHeader, BlockWithTxids, Transaction, Txid } from "../types/block";
import { getDefaultClient, SubcoinRpcClient } from "./client";

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
    return this.client.request<Block | null>("blockchain_getBlock", hash ? [hash] : []);
  }

  /**
   * Get full block by height
   */
  async getBlockByNumber(height?: number): Promise<Block | null> {
    return this.client.request<Block | null>(
      "blockchain_getBlockByNumber",
      height !== undefined ? [height] : []
    );
  }

  /**
   * Get full block with transaction IDs by hash
   */
  async getBlockWithTxids(hash?: BlockHash): Promise<BlockWithTxids | null> {
    return this.client.request<BlockWithTxids | null>(
      "blockchain_getBlockWithTxids",
      hash ? [hash] : []
    );
  }

  /**
   * Get full block with transaction IDs by height
   */
  async getBlockWithTxidsByNumber(height?: number): Promise<BlockWithTxids | null> {
    return this.client.request<BlockWithTxids | null>(
      "blockchain_getBlockWithTxidsByNumber",
      height !== undefined ? [height] : []
    );
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
    return this.client.request<Transaction | null>("blockchain_getTransaction", [txid]);
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
