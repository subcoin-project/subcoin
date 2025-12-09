/**
 * Address balance information
 */
export interface AddressBalance {
  /** Confirmed balance in satoshis */
  confirmed: number;
  /** Total received in satoshis */
  total_received: number;
  /** Total sent in satoshis */
  total_sent: number;
  /** Number of transactions */
  tx_count: number;
  /** Number of unspent outputs */
  utxo_count: number;
}

/**
 * Transaction in address history
 */
export interface AddressTransaction {
  /** Transaction ID */
  txid: string;
  /** Block height */
  block_height: number;
  /** Net change in satoshis (positive = received, negative = sent) */
  delta: number;
  /** Block timestamp */
  timestamp: number;
}

/**
 * Unspent transaction output
 */
export interface AddressUtxo {
  /** Transaction ID */
  txid: string;
  /** Output index */
  vout: number;
  /** Value in satoshis */
  value: number;
  /** Block height */
  block_height: number;
  /** ScriptPubKey as hex */
  script_pubkey: string;
}

/**
 * Indexer status information
 */
export interface IndexerStatus {
  /** Whether the indexer is currently syncing */
  is_syncing: boolean;
  /** Current indexed block height */
  indexed_height: number;
  /** Target block height (during sync) */
  target_height: number | null;
  /** Sync progress as percentage (0.0 - 100.0) */
  progress_percent: number;
}
