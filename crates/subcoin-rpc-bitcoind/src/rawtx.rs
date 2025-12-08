//! Bitcoin Core compatible raw transaction RPC methods.
//!
//! Implements: getrawtransaction, sendrawtransaction, decoderawtransaction

use crate::error::Error;
use crate::types::{GetRawTransaction, ScriptPubKey, ScriptSig, Vin, Vout};
use bitcoin::consensus::encode::{deserialize_hex, serialize_hex};
use bitcoin::{Address, Amount, BlockHash, Network, OutPoint, Transaction, Txid};
use jsonrpsee::proc_macros::rpc;
use sc_client_api::{AuxStore, BlockBackend, HeaderBackend};
use sp_runtime::traits::Block as BlockT;
use std::marker::PhantomData;
use std::sync::Arc;
use subcoin_network::{NetworkApi, SendTransactionResult};
use subcoin_primitives::{BitcoinTransactionAdapter, TransactionIndex, TxPosition};

/// Bitcoin Core compatible raw transaction RPC API.
#[rpc(client, server)]
pub trait RawTransactionApi {
    /// Return the raw transaction data.
    ///
    /// By default, this call only returns a transaction if it is in the mempool.
    /// If -txindex is enabled, it also returns the transaction if it is found in a block.
    ///
    /// If verbose is false (default), returns hex-encoded serialized transaction.
    /// If verbose is true, returns a JSON object with transaction info.
    #[method(name = "getrawtransaction")]
    async fn get_raw_transaction(
        &self,
        txid: Txid,
        verbose: Option<bool>,
        blockhash: Option<BlockHash>,
    ) -> Result<serde_json::Value, Error>;

    /// Submit a raw transaction (serialized, hex-encoded) to local node and network.
    ///
    /// Returns the transaction hash in hex.
    #[method(name = "sendrawtransaction")]
    async fn send_raw_transaction(
        &self,
        hexstring: String,
        maxfeerate: Option<f64>,
    ) -> Result<SendTransactionResult, Error>;

    /// Return a JSON object representing the serialized, hex-encoded transaction.
    #[method(name = "decoderawtransaction", blocking)]
    fn decode_raw_transaction(
        &self,
        hexstring: String,
        iswitness: Option<bool>,
    ) -> Result<serde_json::Value, Error>;
}

/// Bitcoin Core compatible raw transaction RPC implementation.
pub struct RawTransaction<Block, Client, TransactionAdapter> {
    client: Arc<Client>,
    network_api: Arc<dyn NetworkApi>,
    transaction_index: Arc<dyn TransactionIndex + Send + Sync>,
    network: Network,
    _phantom: PhantomData<(Block, TransactionAdapter)>,
}

impl<Block, Client, TransactionAdapter> RawTransaction<Block, Client, TransactionAdapter>
where
    Block: BlockT + 'static,
    Client: HeaderBackend<Block> + BlockBackend<Block> + AuxStore + 'static,
{
    /// Creates a new instance of [`RawTransaction`].
    pub fn new(
        client: Arc<Client>,
        network_api: Arc<dyn NetworkApi>,
        transaction_index: Arc<dyn TransactionIndex + Send + Sync>,
        network: Network,
    ) -> Self {
        Self {
            client,
            network_api,
            transaction_index,
            network,
            _phantom: Default::default(),
        }
    }

    fn best_number(&self) -> u32 {
        use sp_runtime::traits::SaturatedConversion;
        self.client.info().best_number.saturated_into()
    }

    fn calculate_confirmations(&self, block_height: u32) -> i32 {
        let best = self.best_number();
        if block_height > best {
            0
        } else {
            (best - block_height + 1) as i32
        }
    }

    fn tx_to_get_raw_transaction(
        &self,
        tx: &Transaction,
        block_hash: Option<BlockHash>,
        block_height: Option<u32>,
        block_time: Option<u32>,
    ) -> GetRawTransaction {
        let txid = tx.compute_txid();
        let wtxid = tx.compute_wtxid();

        // Calculate sizes
        let size = tx.total_size() as u32;
        let vsize = tx.vsize() as u32;
        let weight = tx.weight().to_wu() as u32;

        // Convert inputs
        let vin: Vec<Vin> = tx
            .input
            .iter()
            .map(|input| {
                if input.previous_output == OutPoint::null() {
                    // Coinbase transaction
                    Vin {
                        txid: None,
                        vout: None,
                        script_sig: None,
                        txinwitness: if !input.witness.is_empty() {
                            Some(input.witness.iter().map(hex::encode).collect())
                        } else {
                            None
                        },
                        sequence: input.sequence.0,
                        coinbase: Some(hex::encode(input.script_sig.as_bytes())),
                    }
                } else {
                    Vin {
                        txid: Some(input.previous_output.txid),
                        vout: Some(input.previous_output.vout),
                        script_sig: Some(ScriptSig {
                            asm: input.script_sig.to_asm_string(),
                            hex: hex::encode(input.script_sig.as_bytes()),
                        }),
                        txinwitness: if !input.witness.is_empty() {
                            Some(input.witness.iter().map(hex::encode).collect())
                        } else {
                            None
                        },
                        sequence: input.sequence.0,
                        coinbase: None,
                    }
                }
            })
            .collect();

        // Convert outputs
        let vout: Vec<Vout> = tx
            .output
            .iter()
            .enumerate()
            .map(|(n, output)| {
                let script = &output.script_pubkey;
                let address = Address::from_script(script, self.network)
                    .ok()
                    .map(|a| a.to_string());

                let script_type = if script.is_p2pkh() {
                    "pubkeyhash"
                } else if script.is_p2sh() {
                    "scripthash"
                } else if script.is_p2wpkh() {
                    "witness_v0_keyhash"
                } else if script.is_p2wsh() {
                    "witness_v0_scripthash"
                } else if script.is_p2tr() {
                    "witness_v1_taproot"
                } else if script.is_op_return() {
                    "nulldata"
                } else {
                    "nonstandard"
                };

                Vout {
                    value: Amount::from_sat(output.value.to_sat()).to_btc(),
                    n: n as u32,
                    script_pub_key: ScriptPubKey {
                        asm: script.to_asm_string(),
                        hex: hex::encode(script.as_bytes()),
                        script_type: script_type.to_string(),
                        address,
                    },
                }
            })
            .collect();

        let confirmations = block_height.map(|h| self.calculate_confirmations(h));

        GetRawTransaction {
            in_active_chain: block_hash.map(|_| true),
            hex: serialize_hex(tx),
            txid,
            hash: Txid::from_raw_hash(*wtxid.as_raw_hash()),
            size,
            vsize,
            weight,
            version: tx.version.0,
            locktime: tx.lock_time.to_consensus_u32(),
            vin,
            vout,
            blockhash: block_hash,
            confirmations,
            blocktime: block_time,
            time: block_time,
        }
    }
}

#[async_trait::async_trait]
impl<Block, Client, TransactionAdapter> RawTransactionApiServer
    for RawTransaction<Block, Client, TransactionAdapter>
where
    Block: BlockT + 'static,
    Client: HeaderBackend<Block> + BlockBackend<Block> + AuxStore + 'static,
    TransactionAdapter: BitcoinTransactionAdapter<Block> + Send + Sync + 'static,
{
    async fn get_raw_transaction(
        &self,
        txid: Txid,
        verbose: Option<bool>,
        _blockhash: Option<BlockHash>,
    ) -> Result<serde_json::Value, Error> {
        // Try to get from mempool first
        if let Some(tx) = self.network_api.get_transaction(txid).await {
            if verbose.unwrap_or(false) {
                let result = self.tx_to_get_raw_transaction(&tx, None, None, None);
                return Ok(serde_json::to_value(result)?);
            } else {
                return Ok(serde_json::Value::String(serialize_hex(&tx)));
            }
        }

        // Try to get from blockchain using tx index
        let Some(TxPosition {
            block_number,
            index,
        }) = self.transaction_index.tx_index(txid)?
        else {
            return Err(Error::TransactionNotFound);
        };

        // Get the block
        let substrate_block_hash = self
            .client
            .hash(block_number.into())?
            .ok_or(Error::BlockNotFound)?;

        let substrate_block = self
            .client
            .block(substrate_block_hash)?
            .ok_or(Error::BlockNotFound)?
            .block;

        let bitcoin_block =
            subcoin_primitives::convert_to_bitcoin_block::<Block, TransactionAdapter>(
                substrate_block,
            )
            .map_err(Error::Header)?;

        let tx = bitcoin_block
            .txdata
            .into_iter()
            .nth(index as usize)
            .ok_or(Error::TransactionNotFound)?;

        let block_hash = bitcoin_block.header.block_hash();

        if verbose.unwrap_or(false) {
            let result = self.tx_to_get_raw_transaction(
                &tx,
                Some(block_hash),
                Some(block_number),
                Some(bitcoin_block.header.time),
            );
            Ok(serde_json::to_value(result)?)
        } else {
            Ok(serde_json::Value::String(serialize_hex(&tx)))
        }
    }

    async fn send_raw_transaction(
        &self,
        hexstring: String,
        _maxfeerate: Option<f64>,
    ) -> Result<SendTransactionResult, Error> {
        if !self.network_api.enabled() {
            return Err(Error::NetworkUnavailable);
        }

        let tx: Transaction = deserialize_hex(&hexstring)?;
        Ok(self.network_api.send_transaction(tx).await)
    }

    fn decode_raw_transaction(
        &self,
        hexstring: String,
        _iswitness: Option<bool>,
    ) -> Result<serde_json::Value, Error> {
        let tx: Transaction = deserialize_hex(&hexstring)?;
        let result = self.tx_to_get_raw_transaction(&tx, None, None, None);
        Ok(serde_json::to_value(result)?)
    }
}
