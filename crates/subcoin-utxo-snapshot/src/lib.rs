mod compressor;
mod serialize;

use self::compressor::ScriptCompression;
use self::serialize::{write_compact_size, VarInt};
use bitcoin::consensus::encode::Encodable;
use bitcoin::hashes::Hash;
use bitcoin::BlockHash;
use compressor::compress_amount;
use std::collections::HashSet;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use subcoin_primitives::runtime::Coin;

// Equivalent function in Rust for serializing an OutPoint and Coin
//
// https://github.com/bitcoin/bitcoin/blob/6f9db1ebcab4064065ccd787161bf2b87e03cc1f/src/kernel/coinstats.cpp#L51
pub fn tx_out_ser(outpoint: bitcoin::OutPoint, coin: &Coin) -> bitcoin::io::Result<Vec<u8>> {
    let mut data = Vec::new();

    // Serialize the OutPoint (txid and vout)
    outpoint.consensus_encode(&mut data)?;

    // Serialize the coin's height and coinbase flag
    let height_and_coinbase = (coin.height << 1) | (coin.is_coinbase as u32);
    height_and_coinbase.consensus_encode(&mut data)?;

    let txout = bitcoin::TxOut {
        value: bitcoin::Amount::from_sat(coin.amount),
        script_pubkey: bitcoin::ScriptBuf::from_bytes(coin.script_pubkey.clone()),
    };

    // Serialize the actual UTXO (value and script)
    txout.consensus_encode(&mut data)?;

    Ok(data)
}

/// Represents a UTXO output in the snapshot format.
///
/// A combination of the output index (vout) and associated coin data.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OutputEntry {
    /// The output index within the transaction.
    pub vout: u32,
    /// The coin data associated with this output.
    pub coin: Coin,
}

/// Represents a single UTXO (Unspent Transaction Output) in Bitcoin.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Utxo {
    /// The transaction ID that contains this UTXO.
    pub txid: bitcoin::Txid,
    /// The output index within the transaction.
    pub vout: u32,
    /// The coin data associated with this UTXO (e.g., amount and any relevant metadata).
    pub coin: Coin,
}

const SNAPSHOT_MAGIC_BYTES: [u8; 5] = [b'u', b't', b'x', b'o', 0xff];

#[derive(Debug, Clone, PartialEq, Eq)]
struct SnapshotMetadata {
    version: u16,
    supported_versions: HashSet<u16>,
    network_magic: [u8; 4],
    base_blockhash: [u8; 32],
    coins_count: u64,
}

impl SnapshotMetadata {
    const VERSION: u16 = 2;

    pub fn new(network_magic: [u8; 4], base_blockhash: [u8; 32], coins_count: u64) -> Self {
        let supported_versions = HashSet::from([Self::VERSION]);
        Self {
            version: Self::VERSION,
            supported_versions,
            network_magic,
            base_blockhash,
            coins_count,
        }
    }

    pub fn serialize<W: std::io::Write>(&self, writer: &mut W) -> std::io::Result<()> {
        writer.write_all(&SNAPSHOT_MAGIC_BYTES)?;
        writer.write_all(&self.version.to_le_bytes())?;
        writer.write_all(&self.network_magic)?;
        writer.write_all(&self.base_blockhash)?;
        writer.write_all(&self.coins_count.to_le_bytes())?;
        Ok(())
    }

    pub fn deserialize<R: std::io::Read>(
        reader: &mut R,
        expected_network_magic: &[u8],
    ) -> std::io::Result<Self> {
        use std::io::{Error, ErrorKind};

        let mut magic_bytes = [0; SNAPSHOT_MAGIC_BYTES.len()];
        reader.read_exact(&mut magic_bytes)?;
        if magic_bytes != SNAPSHOT_MAGIC_BYTES {
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!("Invalid UTXO snapshot magic bytes (expected: {SNAPSHOT_MAGIC_BYTES:?}, got: {magic_bytes:?})"),
            ));
        }

        let mut version_bytes = [0; 2];
        reader.read_exact(&mut version_bytes)?;
        let version = u16::from_le_bytes(version_bytes);

        let supported_versions = HashSet::from([Self::VERSION]);
        if !supported_versions.contains(&version) {
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!("Unsupported snapshot version: {version}"),
            ));
        }

        let mut network_magic = [0u8; 4];
        reader.read_exact(&mut network_magic)?;
        if network_magic != expected_network_magic {
            return Err(Error::new(ErrorKind::InvalidData, "Network magic mismatch"));
        }

        let mut base_blockhash = [0; 32];
        reader.read_exact(&mut base_blockhash)?;

        let mut coins_count_bytes = [0; 8];
        reader.read_exact(&mut coins_count_bytes)?;
        let coins_count = u64::from_le_bytes(coins_count_bytes);

        Ok(Self {
            version,
            supported_versions,
            network_magic,
            base_blockhash,
            coins_count,
        })
    }
}

/// Responsible for dumping the UTXO set snapshot compatible with Bitcoin Core.
pub struct UtxoSnapshotGenerator {
    output_filepath: PathBuf,
    output_file: File,
    network: bitcoin::Network,
}

impl UtxoSnapshotGenerator {
    /// Constructs a new instance of [`UtxoSnapshotGenerator`].
    pub fn new(output_filepath: PathBuf, output_file: File, network: bitcoin::Network) -> Self {
        Self {
            output_filepath,
            output_file,
            network,
        }
    }

    /// Returns the path of output file.
    pub fn path(&self) -> &Path {
        &self.output_filepath
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
        write_snapshot_metadata(
            &mut self.output_file,
            self.network,
            bitcoin_block_hash,
            coins_count,
        )
    }

    /// Write the UTXO snapshot at the specified block to a file.
    ///
    /// NOTE: Do not use it in production.
    pub fn write_utxo_snapshot(
        &mut self,
        bitcoin_block_hash: BlockHash,
        utxos_count: u64,
        utxos: impl IntoIterator<Item = Utxo>,
    ) -> std::io::Result<()> {
        self.write_snapshot_metadata(bitcoin_block_hash, utxos_count)?;

        for (txid, coins) in group_utxos_by_txid(utxos) {
            self.write_coins(txid, coins)?;
        }

        Ok(())
    }

    pub fn write_coins(
        &mut self,
        txid: bitcoin::Txid,
        mut coins: Vec<OutputEntry>,
    ) -> std::io::Result<()> {
        write_coins(&mut self.output_file, txid, coins)
    }
}

/// Groups UTXOs by `txid` and converts them into `(Txid, Vec<OutputEntry>)` format.
///
/// NOTE: this requires substantial RAM.
fn group_utxos_by_txid(
    utxos: impl IntoIterator<Item = Utxo>,
) -> impl IntoIterator<Item = (bitcoin::Txid, Vec<OutputEntry>)> {
    use std::collections::HashMap;

    let mut map: HashMap<bitcoin::Txid, Vec<OutputEntry>> = HashMap::new();

    for utxo in utxos {
        map.entry(utxo.txid)
            .or_insert_with(Vec::new)
            .push(OutputEntry {
                vout: utxo.vout,
                coin: utxo.coin,
            });
    }

    map.into_iter()
}

fn write_snapshot_metadata<W: std::io::Write>(
    writer: &mut W,
    network: bitcoin::Network,
    bitcoin_block_hash: BlockHash,
    coins_count: u64,
) -> std::io::Result<()> {
    let snapshot_metadata = SnapshotMetadata::new(
        network.magic().to_bytes(),
        bitcoin_block_hash.to_byte_array(),
        coins_count,
    );

    snapshot_metadata.serialize(writer)?;

    Ok(())
}

/// Write the UTXO snapshot at the specified block to a file.
///
/// NOTE: Do not use it in production.
fn write_utxo_snapshot<W: std::io::Write>(
    writer: &mut W,
    network: bitcoin::Network,
    bitcoin_block_hash: BlockHash,
    utxos_count: u64,
    utxos: impl IntoIterator<Item = Utxo>,
) -> std::io::Result<()> {
    write_snapshot_metadata(writer, network, bitcoin_block_hash, utxos_count)?;

    for (txid, coins) in group_utxos_by_txid(utxos) {
        write_coins(writer, txid, coins)?;
    }

    Ok(())
}

fn write_coins<W: std::io::Write>(
    writer: &mut W,
    txid: bitcoin::Txid,
    mut coins: Vec<OutputEntry>,
) -> std::io::Result<()> {
    coins.sort_by_key(|output_entry| output_entry.vout);

    let mut data = Vec::new();
    txid.consensus_encode(&mut data)?;

    writer.write(&data)?;

    write_compact_size(writer, coins.len() as u64)?;

    for OutputEntry { vout, coin } in coins {
        write_compact_size(writer, vout as u64)?;
        serialize_coin(writer, coin)?;
    }

    Ok(())
}

fn serialize_coin<W: std::io::Write>(writer: &mut W, coin: Coin) -> std::io::Result<()> {
    let Coin {
        is_coinbase,
        amount,
        height,
        script_pubkey,
    } = coin;

    let code = height as u64 * 2u64 + if is_coinbase { 1u64 } else { 0u64 };

    VarInt(code).serialize(writer)?;
    VarInt(compress_amount(amount)).serialize(writer)?;
    ScriptCompression(script_pubkey).serialize(writer)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snapshot_generation() {
        let block_hash1: BlockHash =
            "00000000839a8e6886ab5951d76f411475428afc90947ee320161bbf18eb6048"
                .parse()
                .unwrap();
        let snapshot_metadata = SnapshotMetadata::new(
            bitcoin::Network::Bitcoin.magic().to_bytes(),
            block_hash1.to_byte_array(),
            1,
        );
        let mut data = Vec::new();
        snapshot_metadata.serialize(&mut data).unwrap();

        // Test data fetched via `./build/src/bitcoin-cli -datadir=$DIR -rpcclienttimeout=0 -named dumptxoutset 1_utxo.dat rollback=1`
        #[rustfmt::skip]
        assert_eq!(
            data,
            // Serialized metadata
            vec![
                0x75, 0x74, 0x78, 0x6f, 0xff, 0x02, 0x00, 0xf9,
                0xbe, 0xb4, 0xd9, 0x48, 0x60, 0xeb, 0x18, 0xbf,
                0x1b, 0x16, 0x20, 0xe3, 0x7e, 0x94, 0x90, 0xfc,
                0x8a, 0x42, 0x75, 0x14, 0x41, 0x6f, 0xd7, 0x51,
                0x59, 0xab, 0x86, 0x68, 0x8e, 0x9a, 0x83, 0x00,
                0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00
            ]
        );

        let txid: bitcoin::Txid =
            "0e3e2357e806b6cdb1f70b54c3a3a17b6714ee1f0e68bebb44a74b1efd512098"
                .parse()
                .unwrap();

        let mut data = Vec::new();
        txid.consensus_encode(&mut data).unwrap();

        #[rustfmt::skip]
        assert_eq!(
            data,
            // Serialized txid
            [
                0x98, 0x20, 0x51, 0xfd, 0x1e,
                0x4b, 0xa7, 0x44, 0xbb, 0xbe, 0x68, 0x0e, 0x1f,
                0xee, 0x14, 0x67, 0x7b, 0xa1, 0xa3, 0xc3, 0x54,
                0x0b, 0xf7, 0xb1, 0xcd, 0xb6, 0x06, 0xe8, 0x57,
                0x23, 0x3e, 0x0e
            ]
        );

        let mut data = Vec::new();
        write_compact_size(&mut data, 1).unwrap();
        assert_eq!(data, [0x01]);

        println!("{:02x?}", data);

        let coin = Coin {
            is_coinbase: true,
            amount: 50_0000_0000,
            height: 1,
            script_pubkey: hex::decode("410496b538e853519c726a2c91e61ec11600ae1390813a627c66fb8be7947be63c52da7589379515d4e0a604f8141781e62294721166bf621e73a82cbf2342c858eeac").unwrap()
        };

        let mut data = Vec::new();
        write_utxo_snapshot(
            &mut data,
            bitcoin::Network::Bitcoin,
            block_hash1,
            1,
            vec![Utxo {
                txid,
                vout: 0,
                coin,
            }],
        )
        .unwrap();

        #[rustfmt::skip]
        assert_eq!(
            data,
            [
                0x75, 0x74, 0x78, 0x6f, 0xff, 0x02, 0x00, 0xf9, 0xbe, 0xb4, 0xd9, 0x48, 0x60, 0xeb, 0x18, 0xbf,
                0x1b, 0x16, 0x20, 0xe3, 0x7e, 0x94, 0x90, 0xfc, 0x8a, 0x42, 0x75, 0x14, 0x41, 0x6f, 0xd7, 0x51,
                0x59, 0xab, 0x86, 0x68, 0x8e, 0x9a, 0x83, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x98, 0x20, 0x51, 0xfd, 0x1e, 0x4b, 0xa7, 0x44, 0xbb, 0xbe, 0x68, 0x0e, 0x1f,
                0xee, 0x14, 0x67, 0x7b, 0xa1, 0xa3, 0xc3, 0x54, 0x0b, 0xf7, 0xb1, 0xcd, 0xb6, 0x06, 0xe8, 0x57,
                0x23, 0x3e, 0x0e, 0x01, 0x00, 0x03, 0x32, 0x04, 0x96, 0xb5, 0x38, 0xe8, 0x53, 0x51, 0x9c, 0x72,
                0x6a, 0x2c, 0x91, 0xe6, 0x1e, 0xc1, 0x16, 0x00, 0xae, 0x13, 0x90, 0x81, 0x3a, 0x62, 0x7c, 0x66,
                0xfb, 0x8b, 0xe7, 0x94, 0x7b, 0xe6, 0x3c, 0x52,
            ]
        );
    }
}
