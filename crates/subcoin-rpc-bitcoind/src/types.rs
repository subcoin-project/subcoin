//! Bitcoin Core compatible JSON-RPC response types.
//!
//! These types match the JSON structure returned by Bitcoin Core's RPC API,
//! enabling compatibility with tools like electrs.

use bitcoin::{BlockHash, Txid};
use serde::{Deserialize, Serialize};

/// Response for `getblockchaininfo` RPC.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetBlockchainInfo {
    /// Current network name (main, test, signet, regtest).
    pub chain: String,
    /// The current number of blocks processed in the server.
    pub blocks: u32,
    /// The current number of headers we have validated.
    pub headers: u32,
    /// The hash of the currently best block.
    pub bestblockhash: BlockHash,
    /// The current difficulty.
    pub difficulty: f64,
    /// Estimate of verification progress [0..1].
    pub verificationprogress: f64,
    /// Whether initial block download is complete.
    pub initialblockdownload: bool,
    /// Total amount of work in active chain, in hexadecimal.
    pub chainwork: String,
    /// The estimated size of the block and undo files on disk.
    pub size_on_disk: u64,
    /// If the blocks are subject to pruning.
    pub pruned: bool,
    /// Any network and blockchain warnings.
    pub warnings: Vec<String>,
}

/// Response for `getnetworkinfo` RPC.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetNetworkInfo {
    /// The server version.
    pub version: u32,
    /// The server subversion string.
    pub subversion: String,
    /// The protocol version.
    pub protocolversion: u32,
    /// The services we offer to the network (hex string).
    pub localservices: String,
    /// True if local relay is active.
    pub localrelay: bool,
    /// The time offset.
    pub timeoffset: i64,
    /// Whether p2p networking is enabled.
    pub networkactive: bool,
    /// The number of connections.
    pub connections: u32,
    /// The number of inbound connections.
    pub connections_in: u32,
    /// The number of outbound connections.
    pub connections_out: u32,
    /// Minimum relay fee rate for transactions in BTC/kvB.
    pub relayfee: f64,
    /// Minimum fee rate increment for mempool limiting or replacement in BTC/kvB.
    pub incrementalfee: f64,
    /// List of local addresses.
    pub localaddresses: Vec<LocalAddress>,
    /// Any network and blockchain warnings.
    pub warnings: Vec<String>,
}

/// Local address information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalAddress {
    /// Network address.
    pub address: String,
    /// Network port.
    pub port: u16,
    /// Relative score.
    pub score: u32,
}

/// Response for `getblock` RPC with verbosity=1 (JSON without tx details).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetBlock {
    /// The block hash.
    pub hash: BlockHash,
    /// The number of confirmations.
    pub confirmations: i32,
    /// The block size in bytes.
    pub size: u32,
    /// The block size excluding witness data.
    pub strippedsize: u32,
    /// The block weight.
    pub weight: u32,
    /// The block height or index.
    pub height: u32,
    /// The block version.
    pub version: i32,
    /// The block version formatted in hexadecimal.
    #[serde(rename = "versionHex")]
    pub version_hex: String,
    /// The merkle root.
    pub merkleroot: String,
    /// The transaction ids.
    pub tx: Vec<Txid>,
    /// The block time in UNIX epoch time.
    pub time: u32,
    /// The median block time in UNIX epoch time.
    pub mediantime: u32,
    /// The nonce.
    pub nonce: u32,
    /// The bits.
    pub bits: String,
    /// The difficulty.
    pub difficulty: f64,
    /// Expected number of hashes required to produce the current chain.
    pub chainwork: String,
    /// The number of transactions in the block.
    #[serde(rename = "nTx")]
    pub n_tx: u32,
    /// The hash of the previous block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previousblockhash: Option<BlockHash>,
    /// The hash of the next block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nextblockhash: Option<BlockHash>,
}

/// Response for `getblockheader` RPC with verbose=true.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetBlockHeader {
    /// The block hash.
    pub hash: BlockHash,
    /// The number of confirmations.
    pub confirmations: i32,
    /// The block height or index.
    pub height: u32,
    /// The block version.
    pub version: i32,
    /// The block version formatted in hexadecimal.
    #[serde(rename = "versionHex")]
    pub version_hex: String,
    /// The merkle root.
    pub merkleroot: String,
    /// The block time in UNIX epoch time.
    pub time: u32,
    /// The median block time in UNIX epoch time.
    pub mediantime: u32,
    /// The nonce.
    pub nonce: u32,
    /// The bits.
    pub bits: String,
    /// The difficulty.
    pub difficulty: f64,
    /// Expected number of hashes required to produce the current chain.
    pub chainwork: String,
    /// The number of transactions in the block.
    #[serde(rename = "nTx")]
    pub n_tx: u32,
    /// The hash of the previous block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previousblockhash: Option<BlockHash>,
    /// The hash of the next block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nextblockhash: Option<BlockHash>,
}

/// Response for `getmempoolinfo` RPC.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetMempoolInfo {
    /// True if the mempool is fully loaded.
    pub loaded: bool,
    /// Current tx count.
    pub size: u64,
    /// Sum of all virtual transaction sizes.
    pub bytes: u64,
    /// Total memory usage for the mempool.
    pub usage: u64,
    /// Total fees for all transactions in the mempool in BTC.
    pub total_fee: f64,
    /// Maximum memory usage for the mempool.
    pub maxmempool: u64,
    /// Minimum fee rate in BTC/kvB for tx to be accepted.
    pub mempoolminfee: f64,
    /// Current minimum relay fee for transactions.
    pub minrelaytxfee: f64,
    /// Minimum fee rate increment for mempool limiting or replacement in BTC/kvB.
    pub incrementalrelayfee: f64,
    /// Current number of transactions that haven't passed initial broadcast yet.
    pub unbroadcastcount: u64,
    /// True if mempool accepts RBF without signaling inspection.
    pub fullrbf: bool,
}

/// Response for `getmempoolentry` RPC.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetMempoolEntry {
    /// Virtual transaction size.
    pub vsize: u64,
    /// Transaction weight.
    pub weight: u64,
    /// Transaction fee in BTC.
    pub fee: f64,
    /// Transaction fee with fee deltas used for mining priority in BTC.
    pub modifiedfee: f64,
    /// Local time transaction entered pool in seconds since 1 Jan 1970 GMT.
    pub time: i64,
    /// Block height when transaction entered pool.
    pub height: u32,
    /// Number of in-mempool descendant transactions (including this one).
    pub descendantcount: u64,
    /// Virtual transaction size of in-mempool descendants (including this one).
    pub descendantsize: u64,
    /// Modified fees of in-mempool descendants (including this one) in BTC.
    pub descendantfees: u64,
    /// Number of in-mempool ancestor transactions (including this one).
    pub ancestorcount: u64,
    /// Virtual transaction size of in-mempool ancestors (including this one).
    pub ancestorsize: u64,
    /// Modified fees of in-mempool ancestors (including this one) in BTC.
    pub ancestorfees: u64,
    /// Hash of serialized transaction, including witness data.
    pub wtxid: Txid,
    /// Unconfirmed transactions used as inputs for this transaction.
    pub depends: Vec<Txid>,
    /// Unconfirmed transactions spending outputs from this transaction.
    pub spentby: Vec<Txid>,
    /// Whether this transaction could be replaced due to BIP125.
    #[serde(rename = "bip125-replaceable")]
    pub bip125_replaceable: bool,
    /// Whether this transaction is currently unbroadcast.
    pub unbroadcast: bool,
}

/// Response for `estimatesmartfee` RPC.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EstimateSmartFee {
    /// Estimate fee rate in BTC/kvB.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feerate: Option<f64>,
    /// Errors encountered during processing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errors: Option<Vec<String>>,
    /// Block number where estimate was found.
    pub blocks: u32,
}

/// Response for `getrawtransaction` RPC with verbose=true.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetRawTransaction {
    /// Whether specified block is in the active chain or not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_active_chain: Option<bool>,
    /// The serialized, hex-encoded data for the transaction.
    pub hex: String,
    /// The transaction id.
    pub txid: Txid,
    /// The transaction hash (differs from txid for witness transactions).
    pub hash: Txid,
    /// The serialized transaction size.
    pub size: u32,
    /// The virtual transaction size.
    pub vsize: u32,
    /// The transaction's weight.
    pub weight: u32,
    /// The version.
    pub version: i32,
    /// The lock time.
    pub locktime: u32,
    /// The transaction inputs.
    pub vin: Vec<Vin>,
    /// The transaction outputs.
    pub vout: Vec<Vout>,
    /// The block hash.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blockhash: Option<BlockHash>,
    /// Number of confirmations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmations: Option<i32>,
    /// The block time in UNIX epoch time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocktime: Option<u32>,
    /// Same as blocktime.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time: Option<u32>,
}

/// Transaction input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vin {
    /// The transaction id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub txid: Option<Txid>,
    /// The output number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vout: Option<u32>,
    /// The script.
    #[serde(rename = "scriptSig", skip_serializing_if = "Option::is_none")]
    pub script_sig: Option<ScriptSig>,
    /// Hex-encoded witness data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub txinwitness: Option<Vec<String>>,
    /// The script sequence number.
    pub sequence: u32,
    /// Coinbase data (for coinbase transactions).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coinbase: Option<String>,
}

/// Script signature.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptSig {
    /// The assembly representation.
    pub asm: String,
    /// The hex representation.
    pub hex: String,
}

/// Transaction output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vout {
    /// The value in BTC.
    pub value: f64,
    /// Index.
    pub n: u32,
    /// The script pubkey.
    #[serde(rename = "scriptPubKey")]
    pub script_pub_key: ScriptPubKey,
}

/// Script pubkey.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptPubKey {
    /// The assembly representation.
    pub asm: String,
    /// The raw output script bytes, hex-encoded.
    pub hex: String,
    /// The type (e.g., pubkeyhash, scripthash, witness_v0_keyhash).
    #[serde(rename = "type")]
    pub script_type: String,
    /// The Bitcoin address (if available).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
}
