import {
  createContext,
  useContext,
  useEffect,
  useState,
  useCallback,
  ReactNode,
} from "react";
import {
  getWebSocketClient,
  connectWebSocket,
  setRpcTarget,
  ConnectionState,
  ConnectionStatus,
} from "@subcoin/shared";

const STORAGE_KEY = "subcoin-endpoint";
const DEFAULT_ENDPOINT = "localhost:9944";

function getStoredEndpoint(): string {
  if (typeof window !== "undefined") {
    return localStorage.getItem(STORAGE_KEY) || DEFAULT_ENDPOINT;
  }
  return DEFAULT_ENDPOINT;
}

interface ConnectionContextValue {
  state: ConnectionState;
  isConnected: boolean;
  endpoint: string;
  connect: () => void;
  disconnect: () => void;
  setEndpoint: (endpoint: string) => void;
}

const ConnectionContext = createContext<ConnectionContextValue | null>(null);

export function ConnectionProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<ConnectionState>({
    status: "disconnected",
  });
  const [endpoint, setEndpointState] = useState<string>(getStoredEndpoint);

  useEffect(() => {
    const ws = getWebSocketClient();

    // Subscribe to connection state changes
    const unsubscribe = ws.onConnectionChange((newState) => {
      setState(newState);
    });

    // Configure endpoints and connect
    const storedEndpoint = getStoredEndpoint();
    // Set RPC target for HTTP proxy (Vite routes /rpc?target=host:port)
    setRpcTarget(storedEndpoint);
    // Set WebSocket endpoint
    ws.setEndpoint(`ws://${storedEndpoint}`);
    connectWebSocket();

    return () => {
      unsubscribe();
    };
  }, []);

  const connect = useCallback(() => {
    connectWebSocket();
  }, []);

  const disconnect = useCallback(() => {
    getWebSocketClient().disconnect();
  }, []);

  const setEndpoint = useCallback((newEndpoint: string) => {
    // Store in localStorage
    localStorage.setItem(STORAGE_KEY, newEndpoint);
    setEndpointState(newEndpoint);

    // Update RPC target for HTTP proxy
    setRpcTarget(newEndpoint);

    // Update WebSocket client
    const ws = getWebSocketClient();
    ws.setEndpoint(`ws://${newEndpoint}`);
  }, []);

  const value: ConnectionContextValue = {
    state,
    isConnected: state.status === "connected",
    endpoint,
    connect,
    disconnect,
    setEndpoint,
  };

  return (
    <ConnectionContext.Provider value={value}>
      {children}
    </ConnectionContext.Provider>
  );
}

export function useConnection(): ConnectionContextValue {
  const context = useContext(ConnectionContext);
  if (!context) {
    throw new Error("useConnection must be used within a ConnectionProvider");
  }
  return context;
}

/**
 * Hook to subscribe to new block notifications
 */
export function useNewBlockSubscription(
  onNewBlock: (blockNumber: number) => void
): void {
  useEffect(() => {
    const ws = getWebSocketClient();
    const unsubscribe = ws.onNewBlock((blockNumber) => {
      onNewBlock(blockNumber);
    });

    return () => {
      unsubscribe();
    };
  }, [onNewBlock]);
}
