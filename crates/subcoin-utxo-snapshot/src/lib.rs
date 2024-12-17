mod compressor;
mod script;
mod serialize;

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
    pub fn write_utxo_snapshot_in_memory(
        &mut self,
        bitcoin_block_hash: BlockHash,
        utxos_count: u64,
        utxos: impl IntoIterator<Item = Utxo>,
    ) -> std::io::Result<()> {
        self.write_snapshot_metadata(bitcoin_block_hash, utxos_count)?;

        let sorted_coins = group_utxos_by_txid(utxos).into_iter().collect::<Vec<_>>();
        for (txid, coins) in sorted_coins {
            self.write_coins(txid, coins)?;
        }

        Ok(())
    }

    pub fn write_coins(
        &mut self,
        txid: bitcoin::Txid,
        coins: Vec<OutputEntry>,
    ) -> std::io::Result<()> {
        write_coins(&mut self.output_file, txid, coins)
    }
}

/// Groups UTXOs by `txid` and converts them into `(Txid, Vec<OutputEntry>)` format.
///
/// NOTE: this requires substantial RAM.
pub fn group_utxos_by_txid(
    utxos: impl IntoIterator<Item = Utxo>,
) -> BTreeMap<bitcoin::Txid, Vec<OutputEntry>> {
    // Use BTreeMap to obtain the same lexicographic order of UTXOs as Bitcoin Core.
    let mut map: BTreeMap<bitcoin::Txid, Vec<OutputEntry>> = BTreeMap::new();

    for utxo in utxos {
        map.entry(utxo.txid)
            .or_insert_with(Vec::new)
            .push(OutputEntry {
                vout: utxo.vout,
                coin: utxo.coin,
            });
    }

    map
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
fn write_utxo_snapshot_in_memory<W: std::io::Write>(
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

    let code = height * 2u32 + u32::from(is_coinbase);

    let mut data1 = Vec::new();
    VarInt::new(code as u64).consensus_encode(&mut data1)?;

    let mut data2 = Vec::new();
    VarInt::new(compress_amount(amount)).consensus_encode(&mut data2)?;
    tracing::info!("[serialize_coin] height: {height}, is_coinbase: {is_coinbase}, amount: {amount}, seralized code: {data1:?}, serialized amount: {data2:?}");

    writer.write_all(&data1)?;
    writer.write_all(&data2)?;
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
        write_utxo_snapshot_in_memory(
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

    fn print_hex_dump(data: &[u8]) {
        for (i, chunk) in data.chunks(16).enumerate() {
            // Print the offset
            print!("{:08x}  ", i * 16);

            // Print the hex values
            for byte in chunk.iter() {
                print!("{:02x} ", byte);
            }

            // Add spacing if the line is not full
            for _ in 0..(16 - chunk.len()) {
                print!("   ");
            }

            // Print the ASCII representation
            print!(" |");
            for byte in chunk {
                if byte.is_ascii_graphic() || *byte == b' ' {
                    print!("{}", *byte as char);
                } else {
                    print!(".");
                }
            }
            println!("|");
        }
    }

    pub(crate) fn parse_csv_entry(line: &str) -> Utxo {
        let parts = line.split(',').collect::<Vec<_>>();
        let (txid, vout) = parts[0].split_once(':').unwrap();
        let is_coinbase = parts[1] == "true";
        let height: u32 = parts[2].parse().unwrap();
        let amount: u64 = parts[3].parse().unwrap();
        let script_pubkey = hex::decode(parts[4].as_bytes()).unwrap();
        Utxo {
            txid: txid.parse().unwrap(),
            vout: vout.parse().unwrap(),
            coin: Coin {
                is_coinbase,
                amount,
                height,
                script_pubkey,
            },
        }
    }

    #[test]
    fn test_snapshot_at_block_6() {
        // subcoin blockchain dumptxoutset --height 6
        let lines = vec![
        "0e3e2357e806b6cdb1f70b54c3a3a17b6714ee1f0e68bebb44a74b1efd512098:0,true,1,5000000000,410496b538e853519c726a2c91e61ec11600ae1390813a627c66fb8be7947be63c52da7589379515d4e0a604f8141781e62294721166bf621e73a82cbf2342c858eeac",
"20251a76e64e920e58291a30d4b212939aae976baca40e70818ceaa596fb9d37:0,true,6,5000000000,410408ce279174b34c077c7b2043e3f3d45a588b85ef4ca466740f848ead7fb498f0a795c982552fdfa41616a7c0333a269d62108588e260fd5a48ac8e4dbf49e2bcac",
"63522845d294ee9b0188ae5cac91bf389a0c3723f084ca1025e7d9cdfe481ce1:0,true,5,5000000000,410456579536d150fbce94ee62b47db2ca43af0a730a0467ba55c79e2a7ec9ce4ad297e35cdbb8e42a4643a60eef7c9abee2f5822f86b1da242d9c2301c431facfd8ac",
"999e1c837c76a1b7fbb7e57baf87b309960f5ffefbf2a9b95dd890602272f644:0,true,3,5000000000,410494b9d3e76c5b1629ecf97fff95d7a4bbdac87cc26099ada28066c6ff1eb9191223cd897194a08d0c2726c5747f1db49e8cf90e75dc3e3550ae9b30086f3cd5aaac",
"9b0fc92260312ce44e74ef369f5c66bbb85848f2eddd5a7a1cde251e54ccfdd5:0,true,2,5000000000,41047211a824f55b505228e4c3d5194c1fcfaa15a456abdf37f9b9d97a4040afc073dee6c89064984f03385237d92167c13e236446b417ab79a0fcae412ae3316b77ac",
"df2b060fa2e5e9c8ed5eaf6a45c13753ec8c63282b2688322eba40cd98ea067a:0,true,4,5000000000,4104184f32b212815c6e522e66686324030ff7e5bf08efb21f8b00614fb7690e19131dd31304c54f37baa40db231c918106bb9fd43373e37ae31a0befc6ecaefb867ac"];

        let utxos = lines.into_iter().map(parse_csv_entry).collect::<Vec<_>>();

        let mut data = Vec::new();
        let block_hash6: BlockHash =
            "000000003031a0e73735690c5a1ff2a4be82553b2a12b776fbd3a215dc8f778d"
                .parse()
                .unwrap();
        let utxos_count = 6;
        write_utxo_snapshot_in_memory(
            &mut data,
            bitcoin::Network::Bitcoin,
            block_hash6,
            utxos_count,
            utxos,
        )
        .unwrap();
        print_hex_dump(&data);
        // println!("{:02x?}", data);
    }

    #[test]
    fn test_varint() {
        let mut data = Vec::new();
        let height = 733953;
        let code: u32 = 2 * height;
        VarInt::new(code as u64)
            .consensus_encode(&mut data)
            .unwrap();
        println!("varint: {data:?}");
    }

    #[test]
    fn test_txid() {
        let txid: bitcoin::Txid =
            "beade4fb4c7e7c7e8b885b7e7c4b176500da5d561a876c5b40eb4f01738d0100"
                .parse()
                .unwrap();
        let mut data = Vec::new();
        txid.consensus_encode(&mut data).unwrap();
        println!("{:x?}", data);
    }

    #[test]
    fn test_code() {
        let height = 733953;
        let code = height * 2u32 + u32::from(false);

        let mut data = Vec::new();
        VarInt::new(code as u64)
            .consensus_encode(&mut data)
            .unwrap();
        println!("{:x?}", data);

        let mut data = Vec::new();
        VarInt::new(143).consensus_encode(&mut data).unwrap();
        println!("143 encoded bytes: {:02x?}", data);

        let mut data = Vec::new();
        VarInt::new(2049).consensus_encode(&mut data).unwrap();
        println!("2049 encoded bytes: {:02x?}", data);

        use bitcoin::consensus::Decodable;
        let mut data = vec![0x80, 0x0f];
        println!(
            "{:?}",
            VarInt::consensus_decode(&mut data.as_slice()).unwrap()
        );
    }
}
