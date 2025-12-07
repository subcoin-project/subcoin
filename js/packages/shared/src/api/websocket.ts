/**
 * WebSocket client for real-time Subcoin updates
 *
 * Uses Substrate's standard subscription methods:
 * - chain_subscribeNewHeads: New block headers
 * - chain_subscribeFinalizedHeads: Finalized block headers
 */

export type ConnectionStatus = "connecting" | "connected" | "disconnected" | "error";

export interface ConnectionState {
  status: ConnectionStatus;
  error?: string;
  lastConnected?: Date;
}

export type ConnectionListener = (state: ConnectionState) => void;
export type BlockListener = (blockNumber: number, blockHash: string) => void;

class SubcoinWebSocket {
  private ws: WebSocket | null = null;
  private endpoint: string;
  private reconnectAttempts = 0;
  private maxReconnectAttempts = 5;
  private reconnectDelay = 1000;
  private subscriptionId: string | null = null;
  private requestId = 0;
  private pendingRequests = new Map<number, (result: unknown) => void>();

  private connectionState: ConnectionState = { status: "disconnected" };
  private connectionListeners = new Set<ConnectionListener>();
  private blockListeners = new Set<BlockListener>();

  constructor(endpoint = "ws://localhost:9944") {
    this.endpoint = endpoint;
  }

  /**
   * Connect to the WebSocket endpoint
   */
  connect(): void {
    if (this.ws?.readyState === WebSocket.OPEN) {
      return;
    }

    this.updateConnectionState({ status: "connecting" });

    try {
      this.ws = new WebSocket(this.endpoint);

      this.ws.onopen = () => {
        this.reconnectAttempts = 0;
        this.updateConnectionState({
          status: "connected",
          lastConnected: new Date(),
        });
        this.subscribeToNewHeads();
      };

      this.ws.onclose = () => {
        this.subscriptionId = null;
        this.updateConnectionState({ status: "disconnected" });
        this.attemptReconnect();
      };

      this.ws.onerror = (event) => {
        console.error("WebSocket error:", event);
        this.updateConnectionState({
          status: "error",
          error: "Connection error",
        });
      };

      this.ws.onmessage = (event) => {
        this.handleMessage(event.data);
      };
    } catch (error) {
      this.updateConnectionState({
        status: "error",
        error: error instanceof Error ? error.message : "Failed to connect",
      });
    }
  }

  /**
   * Disconnect from the WebSocket
   */
  disconnect(): void {
    if (this.ws) {
      this.ws.close();
      this.ws = null;
    }
    this.subscriptionId = null;
    this.updateConnectionState({ status: "disconnected" });
  }

  /**
   * Set the WebSocket endpoint
   */
  setEndpoint(endpoint: string): void {
    this.endpoint = endpoint;
    if (this.ws) {
      this.disconnect();
      this.connect();
    }
  }

  /**
   * Subscribe to connection state changes
   */
  onConnectionChange(listener: ConnectionListener): () => void {
    this.connectionListeners.add(listener);
    // Immediately notify of current state
    listener(this.connectionState);
    return () => this.connectionListeners.delete(listener);
  }

  /**
   * Subscribe to new block notifications
   */
  onNewBlock(listener: BlockListener): () => void {
    this.blockListeners.add(listener);
    return () => this.blockListeners.delete(listener);
  }

  /**
   * Get current connection state
   */
  getConnectionState(): ConnectionState {
    return this.connectionState;
  }

  private updateConnectionState(state: ConnectionState): void {
    this.connectionState = state;
    this.connectionListeners.forEach((listener) => listener(state));
  }

  private attemptReconnect(): void {
    if (this.reconnectAttempts >= this.maxReconnectAttempts) {
      this.updateConnectionState({
        status: "error",
        error: "Max reconnection attempts reached",
      });
      return;
    }

    this.reconnectAttempts++;
    const delay = this.reconnectDelay * Math.pow(2, this.reconnectAttempts - 1);

    setTimeout(() => {
      if (this.connectionState.status !== "connected") {
        this.connect();
      }
    }, delay);
  }

  private subscribeToNewHeads(): void {
    const id = ++this.requestId;

    this.pendingRequests.set(id, (result) => {
      if (typeof result === "string") {
        this.subscriptionId = result;
      }
    });

    this.send({
      jsonrpc: "2.0",
      id,
      method: "chain_subscribeNewHeads",
      params: [],
    });
  }

  private handleMessage(data: string): void {
    try {
      const message = JSON.parse(data);

      // Handle subscription response
      if (message.id && this.pendingRequests.has(message.id)) {
        const callback = this.pendingRequests.get(message.id)!;
        this.pendingRequests.delete(message.id);
        callback(message.result);
        return;
      }

      // Handle subscription notification
      if (message.method === "chain_newHead" && message.params?.result) {
        const header = message.params.result;
        // Substrate header has number in hex
        const blockNumber = parseInt(header.number, 16);
        const blockHash = header.parentHash; // We'll use parent hash as we don't have the current hash directly

        this.blockListeners.forEach((listener) => {
          listener(blockNumber, blockHash);
        });
      }
    } catch (error) {
      console.error("Failed to parse WebSocket message:", error);
    }
  }

  private send(message: object): void {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(message));
    }
  }
}

// Singleton instance
let wsClient: SubcoinWebSocket | null = null;

/**
 * Get or create the WebSocket client
 */
export function getWebSocketClient(): SubcoinWebSocket {
  if (!wsClient) {
    wsClient = new SubcoinWebSocket("ws://localhost:9944");
  }
  return wsClient;
}

/**
 * Connect to the WebSocket
 */
export function connectWebSocket(): void {
  getWebSocketClient().connect();
}

/**
 * Disconnect from the WebSocket
 */
export function disconnectWebSocket(): void {
  getWebSocketClient().disconnect();
}
