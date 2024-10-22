use bitcoin::consensus::encode::Encodable;
use bitcoin::BlockHash;
use std::fs::File;
use std::io::Write;
use subcoin_primitives::runtime::Coin;

/// Responsible for dumping the UTXO set snapshot compatible with Bitcoin Core.
pub struct UtxoSetBinaryOutput {
    output_file: File,
}

impl UtxoSetBinaryOutput {
    /// Constructs a new instance of [`UtxoSetBinaryOutput`].
    pub fn new(output_file: File) -> Self {
        Self { output_file }
    }

    /// Writes a single entry of UTXO.
    pub fn write_utxo_entry(
        &mut self,
        txid: bitcoin::Txid,
        vout: u32,
        coin: Coin,
    ) -> std::io::Result<()> {
        let Coin {
            is_coinbase,
            amount,
            height,
            script_pubkey,
        } = coin;

        let outpoint = bitcoin::OutPoint { txid, vout };

        let mut data = Vec::new();

        let amount = txoutset::Amount::new(amount);

        let code = txoutset::Code {
            height,
            is_coinbase,
        };
        let script = txoutset::Script::from_bytes(script_pubkey);

        outpoint.consensus_encode(&mut data)?;
        code.consensus_encode(&mut data)?;
        amount.consensus_encode(&mut data)?;
        script.consensus_encode(&mut data)?;

        let _ = self.output_file.write(data.as_slice())?;

        Ok(())
    }

    /// Writes the metadata of snapshot.
    pub fn write_snapshot_metadata(
        &mut self,
        bitcoin_block_hash: BlockHash,
        coins_count: u64,
    ) -> std::io::Result<()> {
        let mut data = Vec::new();

        bitcoin_block_hash
            .consensus_encode(&mut data)
            .expect("Failed to encode");

        coins_count
            .consensus_encode(&mut data)
            .expect("Failed to write utxo set size");

        let _ = self.output_file.write(data.as_slice())?;

        Ok(())
    }

    /// Write the UTXO snapshot at the specified block to a file.
    pub fn write_utxo_snapshot(
        &mut self,
        bitcoin_block_hash: BlockHash,
        utxos: Vec<(bitcoin::Txid, u32, Coin)>,
    ) -> std::io::Result<()> {
        self.write_snapshot_metadata(bitcoin_block_hash, utxos.len() as u64)?;

        for (txid, vout, coin) in utxos {
            self.write_utxo_entry(txid, vout, coin)?;
        }

        Ok(())
    }
}
