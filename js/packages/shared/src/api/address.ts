/**
 * Address API - wraps address_* RPC methods
 */

import type { AddressBalance, AddressStats, AddressTransaction, AddressUtxo, IndexerStatus, OutputStatus } from "../types/address";
import { getDefaultClient, SubcoinRpcClient } from "./client";

export class AddressApi {
  constructor(private client: SubcoinRpcClient = getDefaultClient()) {}

  /**
   * Get address balance
   */
  async getBalance(address: string): Promise<AddressBalance> {
    return this.client.request<AddressBalance>("address_getBalance", [address]);
  }

  /**
   * Get address transaction history with pagination
   */
  async getHistory(
    address: string,
    limit?: number,
    offset?: number
  ): Promise<AddressTransaction[]> {
    return this.client.request<AddressTransaction[]>("address_getHistory", [
      address,
      limit ?? null,
      offset ?? null,
    ]);
  }

  /**
   * Get address UTXOs
   */
  async getUtxos(address: string): Promise<AddressUtxo[]> {
    return this.client.request<AddressUtxo[]>("address_getUtxos", [address]);
  }

  /**
   * Get address transaction count
   */
  async getTxCount(address: string): Promise<number> {
    return this.client.request<number>("address_getTxCount", [address]);
  }

  /**
   * Get indexer status (sync progress)
   */
  async getIndexerStatus(): Promise<IndexerStatus> {
    return this.client.request<IndexerStatus>("address_indexerStatus", []);
  }

  /**
   * Get address statistics (first/last seen, largest tx, etc)
   */
  async getStats(address: string): Promise<AddressStats> {
    return this.client.request<AddressStats>("address_getStats", [address]);
  }

  /**
   * Get output spending status
   */
  async getOutputStatus(txid: string, vout: number): Promise<OutputStatus | null> {
    return this.client.request<OutputStatus | null>("address_getOutputStatus", [txid, vout]);
  }
}

/**
 * Default address API instance
 */
export const addressApi = new AddressApi();
