/**
 * Bitcoin block header
 */
export interface BlockHeader {
  /** Block version */
  version: number;
  /** Previous block hash */
  prev_blockhash: string;
  /** Merkle root of transactions */
  merkle_root: string;
  /** Block timestamp (Unix time) */
  time: number;
  /** Difficulty target (bits) */
  bits: number;
  /** Nonce used for mining */
  nonce: number;
}

/**
 * Bitcoin transaction input
 */
export interface TxIn {
  /** Previous output reference */
  previous_output: {
    txid: string;
    vout: number;
  };
  /** Script signature (hex) */
  script_sig: string;
  /** Sequence number */
  sequence: number;
  /** Witness data (for SegWit transactions) */
  witness?: string[];
}

/**
 * Bitcoin transaction output
 */
export interface TxOut {
  /** Value in satoshis */
  value: number;
  /** Script pubkey (hex) */
  script_pubkey: string;
}

/**
 * Bitcoin transaction
 */
export interface Transaction {
  /** Transaction version */
  version: number;
  /** Lock time */
  lock_time: number;
  /** Transaction inputs */
  input: TxIn[];
  /** Transaction outputs */
  output: TxOut[];
}

/**
 * Full Bitcoin block with header and transactions
 */
export interface Block {
  /** Block header */
  header: BlockHeader;
  /** List of transactions */
  txdata: Transaction[];
}

/**
 * Computed transaction ID from a transaction
 */
export type Txid = string;

/**
 * Block hash (32 bytes hex)
 */
export type BlockHash = string;
