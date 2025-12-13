/**
 * Network API - wraps network_* RPC methods
 */

import type { NetworkStatus, SyncPeers } from "../types/network";
import { getDefaultClient, SubcoinRpcClient } from "./client";

export class NetworkApi {
  constructor(private client: SubcoinRpcClient = getDefaultClient()) {}

  /**
   * Get sync peers information
   */
  async getSyncPeers(): Promise<SyncPeers> {
    return this.client.request<SyncPeers>("network_syncPeers", []);
  }

  /**
   * Get overall network status
   */
  async getStatus(): Promise<NetworkStatus | null> {
    return this.client.request<NetworkStatus | null>("network_status", []);
  }
}

/**
 * Default network API instance
 */
export const networkApi = new NetworkApi();
