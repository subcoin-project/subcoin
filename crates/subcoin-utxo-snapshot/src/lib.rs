mod compressor;
mod script;
mod serialize;
#[cfg(test)]
mod tests;

use self::compressor::ScriptCompression;
use self::serialize::write_compact_size;
use bitcoin::consensus::encode::Encodable;
use bitcoin::hashes::Hash;
use bitcoin::BlockHash;
use compressor::compress_amount;
use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use subcoin_primitives::runtime::Coin;
use txoutset::var_int::VarInt;

const SNAPSHOT_MAGIC_BYTES: [u8; 5] = [b'u', b't', b'x', b'o', 0xff];

/// Groups UTXOs by `txid` into a lexicographically ordered `BTreeMap` (same as the order stored by
/// Bitcoin Core in leveldb).
///
/// NOTE: this requires substantial RAM.
pub fn group_utxos_by_txid(
    utxos: impl IntoIterator<Item = Utxo>,
) -> BTreeMap<bitcoin::Txid, Vec<OutputEntry>> {
    let mut map: BTreeMap<bitcoin::Txid, Vec<OutputEntry>> = BTreeMap::new();

    for utxo in utxos {
        map.entry(utxo.txid).or_default().push(OutputEntry {
            vout: utxo.vout,
            coin: utxo.coin,
        });
    }

    map
}

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

impl From<(bitcoin::Txid, u32, Coin)> for Utxo {
    fn from((txid, vout, coin): (bitcoin::Txid, u32, Coin)) -> Self {
        Self { txid, vout, coin }
    }
}

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

    #[allow(unused)]
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
    pub fn generate_snapshot_in_mem(
        &mut self,
        bitcoin_block_hash: BlockHash,
        utxos_count: u64,
        utxos: impl IntoIterator<Item = Utxo>,
    ) -> std::io::Result<()> {
        generate_snapshot_in_mem_inner(
            &mut self.output_file,
            self.network,
            bitcoin_block_hash,
            utxos_count,
            utxos,
        )
    }

    /// Writes UTXO entries for a given transaction.
    pub fn write_coins(
        &mut self,
        txid: bitcoin::Txid,
        coins: Vec<OutputEntry>,
    ) -> std::io::Result<()> {
        write_coins(&mut self.output_file, txid, coins)
    }
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

/// Write the UTXO snapshot at the specified block using the given writer.
///
/// NOTE: Do not use it in production.
fn generate_snapshot_in_mem_inner<W: std::io::Write>(
    writer: &mut W,
    network: bitcoin::Network,
    bitcoin_block_hash: BlockHash,
    utxos_count: u64,
    utxos: impl IntoIterator<Item = Utxo>,
) -> std::io::Result<()> {
    write_snapshot_metadata(writer, network, bitcoin_block_hash, utxos_count)?;

    let sorted_coins = group_utxos_by_txid(utxos).into_iter().collect::<Vec<_>>();
    for (txid, coins) in sorted_coins {
        write_coins(writer, txid, coins)?;
    }

    Ok(())
}

pub fn write_coins<W: std::io::Write>(
    writer: &mut W,
    txid: bitcoin::Txid,
    mut coins: Vec<OutputEntry>,
) -> std::io::Result<()> {
    coins.sort_by_key(|output_entry| output_entry.vout);

    let mut data = Vec::new();
    txid.consensus_encode(&mut data)?;
    writer.write_all(&data)?;

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

    // https://github.com/bitcoin/bitcoin/blob/0903ce8dbc25d3823b03d52f6e6bff74d19e801e/src/coins.h#L62
    let code = (height << 1) | is_coinbase as u32;

    let mut data = Vec::new();
    VarInt::new(code as u64).consensus_encode(&mut data)?;
    VarInt::new(compress_amount(amount)).consensus_encode(&mut data)?;
    writer.write_all(&data)?;

    ScriptCompression(script_pubkey).serialize(writer)?;

    Ok(())
}
