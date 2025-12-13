/**
 * System API - wraps system_* RPC methods (Substrate standard)
 */

import { getDefaultClient, SubcoinRpcClient } from "./client";

export interface SystemHealth {
  peers: number;
  isSyncing: boolean;
  shouldHavePeers: boolean;
}

export interface SystemSyncState {
  startingBlock: number;
  currentBlock: number;
  highestBlock?: number;
}

export class SystemApi {
  constructor(private client: SubcoinRpcClient = getDefaultClient()) {}

  /**
   * Get system health
   */
  async health(): Promise<SystemHealth> {
    return this.client.request<SystemHealth>("system_health", []);
  }

  /**
   * Get node name
   */
  async name(): Promise<string> {
    return this.client.request<string>("system_name", []);
  }

  /**
   * Get node version
   */
  async version(): Promise<string> {
    return this.client.request<string>("system_version", []);
  }

  /**
   * Get chain name
   */
  async chain(): Promise<string> {
    return this.client.request<string>("system_chain", []);
  }

  /**
   * Get sync state
   */
  async syncState(): Promise<SystemSyncState> {
    return this.client.request<SystemSyncState>("system_syncState", []);
  }
}

/**
 * Default system API instance
 */
export const systemApi = new SystemApi();
