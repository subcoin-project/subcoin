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
  ConnectionState,
  ConnectionStatus,
} from "@subcoin/shared";

interface ConnectionContextValue {
  state: ConnectionState;
  isConnected: boolean;
  connect: () => void;
  disconnect: () => void;
}

const ConnectionContext = createContext<ConnectionContextValue | null>(null);

export function ConnectionProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<ConnectionState>({
    status: "disconnected",
  });

  useEffect(() => {
    const ws = getWebSocketClient();

    // Subscribe to connection state changes
    const unsubscribe = ws.onConnectionChange((newState) => {
      setState(newState);
    });

    // Auto-connect on mount
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

  const value: ConnectionContextValue = {
    state,
    isConnected: state.status === "connected",
    connect,
    disconnect,
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
