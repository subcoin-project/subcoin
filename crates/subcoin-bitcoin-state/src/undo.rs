//! Block undo data for chain reorganizations.
//!
//! When a block is applied, we save the UTXOs that were spent and the outpoints
//! that were created. This allows us to revert the block if needed during a reorg.

use crate::Coin;
use bitcoin::OutPoint;
use serde::{Deserialize, Serialize};

/// Undo data for a single block.
///
/// Contains all information needed to revert the block's UTXO changes.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BlockUndo {
    /// UTXOs that were spent in this block.
    /// These need to be restored when reverting.
    pub spent_utxos: Vec<(OutPoint, Coin)>,

    /// Outpoints that were created in this block.
    /// These need to be removed when reverting.
    pub created_outpoints: Vec<OutPoint>,
}

impl BlockUndo {
    /// Create a new empty BlockUndo.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a spent UTXO.
    pub fn record_spend(&mut self, outpoint: OutPoint, coin: Coin) {
        self.spent_utxos.push((outpoint, coin));
    }

    /// Record a created UTXO.
    pub fn record_create(&mut self, outpoint: OutPoint) {
        self.created_outpoints.push(outpoint);
    }

    /// Serialize to bytes for storage.
    pub fn encode(&self) -> Vec<u8> {
        bincode::serialize(self).expect("BlockUndo serialization should not fail")
    }

    /// Deserialize from bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, bincode::Error> {
        bincode::deserialize(bytes)
    }

    /// Returns the number of UTXOs spent in this block.
    pub fn spent_count(&self) -> usize {
        self.spent_utxos.len()
    }

    /// Returns the number of UTXOs created in this block.
    pub fn created_count(&self) -> usize {
        self.created_outpoints.len()
    }

    /// Returns true if no UTXO changes were recorded.
    pub fn is_empty(&self) -> bool {
        self.spent_utxos.is_empty() && self.created_outpoints.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::hashes::Hash;

    #[test]
    fn test_block_undo_roundtrip() {
        let mut undo = BlockUndo::new();

        let outpoint1 = OutPoint {
            txid: bitcoin::Txid::all_zeros(),
            vout: 0,
        };
        let coin1 = Coin::new(true, 5000000000, 0, vec![0x51]);

        let outpoint2 = OutPoint {
            txid: bitcoin::Txid::all_zeros(),
            vout: 1,
        };

        undo.record_spend(outpoint1, coin1.clone());
        undo.record_create(outpoint2);

        let encoded = undo.encode();
        let decoded = BlockUndo::decode(&encoded).unwrap();

        assert_eq!(undo.spent_count(), decoded.spent_count());
        assert_eq!(undo.created_count(), decoded.created_count());
        assert_eq!(undo.spent_utxos[0].0, decoded.spent_utxos[0].0);
        assert_eq!(undo.spent_utxos[0].1, decoded.spent_utxos[0].1);
        assert_eq!(undo.created_outpoints[0], decoded.created_outpoints[0]);
    }
}
