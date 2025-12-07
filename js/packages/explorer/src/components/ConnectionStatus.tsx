import { useConnection } from "../contexts/ConnectionContext";

export function ConnectionStatus() {
  const { state, isConnected, connect } = useConnection();

  const getStatusColor = () => {
    switch (state.status) {
      case "connected":
        return "bg-green-500";
      case "connecting":
        return "bg-yellow-500 animate-pulse";
      case "disconnected":
        return "bg-gray-500";
      case "error":
        return "bg-red-500";
      default:
        return "bg-gray-500";
    }
  };

  const getStatusText = () => {
    switch (state.status) {
      case "connected":
        return "Connected";
      case "connecting":
        return "Connecting...";
      case "disconnected":
        return "Disconnected";
      case "error":
        return state.error || "Error";
      default:
        return "Unknown";
    }
  };

  return (
    <div className="flex items-center space-x-2">
      <div className={`w-2 h-2 rounded-full ${getStatusColor()}`} />
      <span className="text-gray-400 text-sm">{getStatusText()}</span>
      {!isConnected && state.status !== "connecting" && (
        <button
          onClick={connect}
          className="text-xs text-bitcoin-orange hover:underline ml-2"
        >
          Reconnect
        </button>
      )}
    </div>
  );
}
