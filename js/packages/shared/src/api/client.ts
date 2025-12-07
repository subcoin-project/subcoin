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

// The target host:port for dynamic proxy routing
let rpcTarget: string | null = null;

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

    // Build endpoint with optional target param for dynamic proxy routing
    let endpoint = this.endpoint;
    if (rpcTarget && endpoint === "/rpc") {
      endpoint = `/rpc?target=${encodeURIComponent(rpcTarget)}`;
    }

    const response = await fetch(endpoint, {
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
      // Use proxy path - Vite will forward to the target
      endpoint: "/rpc",
    });
  }
  return defaultClient;
}

/**
 * Set the RPC target host:port for dynamic proxy routing
 * This is used by the Vite proxy to route requests to different nodes
 */
export function setRpcTarget(target: string | null): void {
  rpcTarget = target;
}

/**
 * Get the current RPC target
 */
export function getRpcTarget(): string | null {
  return rpcTarget;
}

/**
 * Set the default RPC client endpoint
 */
export function setDefaultEndpoint(endpoint: string): void {
  getDefaultClient().setEndpoint(endpoint);
}
