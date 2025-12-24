//! Coin utilities for native UTXO storage.

use bitcoin::OutPoint;
use bitcoin::hashes::Hash;

// Re-export Coin from runtime primitives.
pub use subcoin_runtime_primitives::Coin;

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

    #[test]
    fn test_coin_roundtrip() {
        let coin = Coin {
            is_coinbase: true,
            amount: 5000000000,
            height: 0,
            script_pubkey: vec![0x51], // OP_TRUE
        };

        let encoded = coin.encode_for_storage();
        let decoded = Coin::decode_from_storage(&encoded).unwrap();

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
