export { SubcoinRpcClient, getDefaultClient, setDefaultEndpoint, setRpcTarget, getRpcTarget } from "./client";
export type { RpcClientConfig } from "./client";

export { AddressApi, addressApi } from "./address";
export { BlockchainApi, blockchainApi } from "./blockchain";
export { NetworkApi, networkApi } from "./network";
export { SystemApi, systemApi } from "./system";
export type { SystemHealth, SystemSyncState } from "./system";

export {
  getWebSocketClient,
  connectWebSocket,
  disconnectWebSocket,
} from "./websocket";
export type {
  ConnectionStatus,
  ConnectionState,
  ConnectionListener,
  BlockListener,
} from "./websocket";
