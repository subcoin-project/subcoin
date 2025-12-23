//! Coin type for native UTXO storage.

use bitcoin::consensus::Encodable;
use bitcoin::hashes::Hash;
use bitcoin::{Amount, OutPoint, ScriptBuf, TxOut};
use serde::{Deserialize, Serialize};

/// Unspent transaction output stored in native storage.
///
/// This is optimized for native RocksDB storage and MuHash computation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Coin {
    /// Whether the coin is from a coinbase transaction.
    pub is_coinbase: bool,
    /// Transfer value in satoshis.
    pub amount: u64,
    /// Block height at which the containing transaction was included.
    pub height: u32,
    /// Spending condition of the output (scriptPubKey).
    pub script_pubkey: Vec<u8>,
}

impl Coin {
    /// Create a new Coin.
    pub fn new(is_coinbase: bool, amount: u64, height: u32, script_pubkey: Vec<u8>) -> Self {
        Self {
            is_coinbase,
            amount,
            height,
            script_pubkey,
        }
    }

    /// Create a Coin from a Bitcoin TxOut.
    pub fn from_txout(txout: &TxOut, height: u32, is_coinbase: bool) -> Self {
        Self {
            is_coinbase,
            amount: txout.value.to_sat(),
            height,
            script_pubkey: txout.script_pubkey.to_bytes(),
        }
    }

    /// Serialize to bytes for RocksDB storage.
    pub fn encode(&self) -> Vec<u8> {
        bincode::serialize(self).expect("Coin serialization should not fail")
    }

    /// Deserialize from bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, bincode::Error> {
        bincode::deserialize(bytes)
    }

    /// Serialize for MuHash computation (Bitcoin Core compatible format).
    ///
    /// Format: OutPoint || (height << 1 | is_coinbase) || TxOut
    ///
    /// Reference: https://github.com/bitcoin/bitcoin/blob/6f9db1e/src/kernel/coinstats.cpp#L51
    pub fn serialize_for_muhash(&self, outpoint: &OutPoint) -> Vec<u8> {
        let mut data = Vec::with_capacity(36 + 4 + 8 + self.script_pubkey.len() + 9);

        // Serialize OutPoint (txid || vout)
        outpoint
            .consensus_encode(&mut data)
            .expect("OutPoint encoding should not fail");

        // Serialize height and coinbase flag: (height << 1) | is_coinbase
        let height_and_coinbase = (self.height << 1) | (self.is_coinbase as u32);
        height_and_coinbase
            .consensus_encode(&mut data)
            .expect("u32 encoding should not fail");

        // Serialize TxOut (value || script)
        let txout = TxOut {
            value: Amount::from_sat(self.amount),
            script_pubkey: ScriptBuf::from_bytes(self.script_pubkey.clone()),
        };
        txout
            .consensus_encode(&mut data)
            .expect("TxOut encoding should not fail");

        data
    }
}

/// Convert OutPoint to storage key (36 bytes).
///
/// Format: txid (32 bytes, raw) || vout (4 bytes, little-endian)
pub fn outpoint_to_key(outpoint: &OutPoint) -> [u8; 36] {
    let mut key = [0u8; 36];
    key[..32].copy_from_slice(outpoint.txid.as_ref());
    key[32..].copy_from_slice(&outpoint.vout.to_le_bytes());
    key
}

/// Parse storage key back to OutPoint.
#[allow(dead_code)]
pub fn key_to_outpoint(key: &[u8; 36]) -> OutPoint {
    let mut txid_bytes = [0u8; 32];
    txid_bytes.copy_from_slice(&key[..32]);
    let txid = bitcoin::Txid::from_byte_array(txid_bytes);
    let vout = u32::from_le_bytes(key[32..].try_into().unwrap());
    OutPoint { txid, vout }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::hashes::Hash;

    #[test]
    fn test_coin_roundtrip() {
        let coin = Coin::new(true, 5000000000, 0, vec![0x51]); // OP_TRUE

        let encoded = coin.encode();
        let decoded = Coin::decode(&encoded).unwrap();

        assert_eq!(coin, decoded);
    }

    #[test]
    fn test_outpoint_key_roundtrip() {
        let outpoint = OutPoint {
            txid: bitcoin::Txid::all_zeros(),
            vout: 42,
        };

        let key = outpoint_to_key(&outpoint);
        let decoded = key_to_outpoint(&key);

        assert_eq!(outpoint, decoded);
    }
}
