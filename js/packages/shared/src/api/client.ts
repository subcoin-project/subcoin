/**
 * Subcoin RPC Client
 *
 * Connects to a Subcoin node via JSON-RPC over HTTP.
 * In development, requests are proxied through Vite to avoid CORS issues.
 */

export interface RpcClientConfig {
  /** RPC endpoint URL (e.g., "/rpc" for proxy or "http://localhost:9944" for direct) */
  endpoint: string;
}

export class SubcoinRpcClient {
  private endpoint: string;
  private requestId = 0;

  constructor(config: RpcClientConfig) {
    this.endpoint = config.endpoint;
  }

  /**
   * Make a JSON-RPC request
   */
  async request<T>(method: string, params: unknown[] = []): Promise<T> {
    const id = ++this.requestId;

    const response = await fetch(this.endpoint, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
      },
      body: JSON.stringify({
        jsonrpc: "2.0",
        id,
        method,
        params,
      }),
    });

    if (!response.ok) {
      throw new Error(`HTTP error: ${response.status} ${response.statusText}`);
    }

    const json = await response.json();

    if (json.error) {
      throw new Error(`RPC error: ${json.error.message} (code: ${json.error.code})`);
    }

    return json.result as T;
  }

  /**
   * Update the endpoint URL
   */
  setEndpoint(endpoint: string): void {
    this.endpoint = endpoint;
  }

  /**
   * Get current endpoint
   */
  getEndpoint(): string {
    return this.endpoint;
  }
}

/**
 * Default RPC client instance
 */
let defaultClient: SubcoinRpcClient | null = null;

/**
 * Get or create the default RPC client
 * Uses "/rpc" proxy in development to avoid CORS issues
 */
export function getDefaultClient(): SubcoinRpcClient {
  if (!defaultClient) {
    defaultClient = new SubcoinRpcClient({
      // Use proxy path - Vite will forward to localhost:9944
      endpoint: "/rpc",
    });
  }
  return defaultClient;
}

/**
 * Set the default RPC client endpoint
 */
export function setDefaultEndpoint(endpoint: string): void {
  getDefaultClient().setEndpoint(endpoint);
}
