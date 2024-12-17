use bitcoin::consensus::Encodable;
use std::io::Read;
use std::vec::Vec;
use txoutset::var_int::VarInt;

const MAX_MONEY: u64 = 21000000 * 100000000;

// Constants for opcodes
const OP_DUP: u8 = 0x76;
const OP_HASH160: u8 = 0xa9;
const OP_EQUALVERIFY: u8 = 0x88;
const OP_CHECKSIG: u8 = 0xac;
const OP_EQUAL: u8 = 0x87;

// https://github.com/bitcoin/bitcoin/blob/0903ce8dbc25d3823b03d52f6e6bff74d19e801e/src/compressor.cpp#L140
//
// NOTE: This function is defined only for 0 <= n <= MAX_MONEY.
pub fn compress_amount(n: u64) -> u64 {
    assert!(n <= MAX_MONEY);

    if n == 0 {
        return 0;
    }
    let mut e = 0;
    let mut n = n;
    while n % 10 == 0 && e < 9 {
        n /= 10;
        e += 1;
    }
    if e < 9 {
        let d = n % 10;
        assert!((1..=9).contains(&d));
        n /= 10;
        1 + (n * 9 + d - 1) * 10 + e as u64
    } else {
        1 + (n - 1) * 10 + 9
    }
}

#[allow(unused)]
pub fn decompress_amount(x: u64) -> u64 {
    if x == 0 {
        return 0;
    }
    let mut x = x - 1;
    let e = x % 10;
    x /= 10;
    let mut n = if e < 9 {
        let d = (x % 9) + 1;
        x /= 9;
        x * 10 + d
    } else {
        x + 1
    };
    for _ in 0..e {
        n *= 10;
    }
    n
}

fn to_key_id(script: &[u8]) -> Option<[u8; 20]> {
    if script.len() == 25
        && script[0] == OP_DUP
        && script[1] == OP_HASH160
        && script[2] == 20
        && script[23] == OP_EQUALVERIFY
        && script[24] == OP_CHECKSIG
    {
        Some(script[3..23].try_into().expect("Size must be 20; qed"))
    } else {
        None
    }
}

fn to_script_id(script: &[u8]) -> Option<[u8; 20]> {
    if script.len() == 23 && script[0] == OP_HASH160 && script[1] == 20 && script[22] == OP_EQUAL {
        Some(script[2..22].try_into().expect("Size must be 20; qed"))
    } else {
        None
    }
}

enum PublicKey {
    Compressed([u8; 33]),
    Uncompressed([u8; 65]),
}

fn to_pub_key(script: &[u8]) -> Option<PublicKey> {
    if script.len() == 35
        && script[0] == 33
        && script[34] == OP_CHECKSIG
        && (script[1] == 0x02 || script[1] == 0x03)
    {
        Some(PublicKey::Compressed(
            script[1..34].try_into().expect("Size must be 33; qed"),
        ))
    } else if script.len() == 67
        && script[0] == 65
        && script[66] == OP_CHECKSIG
        && script[1] == 0x04
    {
        // If not fully valid, it would not be compressible.
        let is_fully_valid = bitcoin::Script::from_bytes(script)
            .p2pk_public_key()
            .is_some();
        if is_fully_valid {
            Some(PublicKey::Uncompressed(
                script[1..66].try_into().expect("Size be 65; qed"),
            ))
        } else {
            None
        }
    } else {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompressedScript(pub Vec<u8>);

pub fn compress_script(script: &[u8]) -> Option<CompressedScript> {
    if let Some(hash) = to_key_id(script) {
        let mut out = Vec::with_capacity(21);
        out.push(0x00);
        out.extend(hash);
        return Some(CompressedScript(out));
    }

    if let Some(hash) = to_script_id(script) {
        let mut out = Vec::with_capacity(21);
        out.push(0x01);
        out.extend(hash);
        return Some(CompressedScript(out));
    }

    if let Some(public_key) = to_pub_key(script) {
        let mut out = Vec::with_capacity(33);

        match public_key {
            PublicKey::Compressed(compressed) => {
                out.extend(compressed);
            }
            PublicKey::Uncompressed(uncompressed) => {
                out.push(0x04 | (uncompressed[64] & 0x01));
                out.extend_from_slice(&uncompressed[1..33]);
            }
        }

        return Some(CompressedScript(out));
    }

    None
}

#[allow(unused)]
fn decompress_script<R: Read>(stream: &mut R) -> std::io::Result<Option<bitcoin::ScriptBuf>> {
    let mut n_size_buf = [0u8; 1];
    stream.read_exact(&mut n_size_buf)?;
    let n_size = n_size_buf[0];

    match n_size {
        0x00 => {
            // P2PKH
            let mut data = [0u8; 20];
            stream.read_exact(&mut data)?;

            let bytes = vec![OP_DUP, OP_HASH160, 20]
                .into_iter()
                .chain(data.iter().cloned())
                .chain(vec![OP_EQUALVERIFY, OP_CHECKSIG])
                .collect();
            Ok(Some(bitcoin::ScriptBuf::from_bytes(bytes)))
        }
        0x01 => {
            // P2SH
            let mut data = [0u8; 20];
            stream.read_exact(&mut data)?;

            let bytes = vec![OP_HASH160, 20]
                .into_iter()
                .chain(data.iter().cloned())
                .chain(vec![OP_EQUAL])
                .collect();
            Ok(Some(bitcoin::ScriptBuf::from_bytes(bytes)))
        }
        0x02 | 0x03 => {
            // Compressed PubKey
            let mut data = [0u8; 32];
            stream.read_exact(&mut data)?;

            let mut bytes = Vec::new();
            bytes.push(33); // Key length
            bytes.push(n_size); // Prefix
            bytes.extend_from_slice(&data);
            bytes.push(OP_CHECKSIG); // OP_CHECKSIG
            Ok(Some(bitcoin::ScriptBuf::from_bytes(bytes)))
        }
        0x04 | 0x05 => {
            let mut compressed_pubkey = [0u8; 33];
            compressed_pubkey[0] = n_size - 2;
            stream.read_exact(&mut compressed_pubkey[1..])?;

            let Ok(pubkey) = bitcoin::PublicKey::from_slice(&compressed_pubkey) else {
                return Ok(None);
            };

            // Uncompressed PubKey
            let uncompressed = pubkey.inner.serialize_uncompressed();

            let mut bytes = Vec::new();
            bytes.push(65); // PubKey length
            bytes.extend(uncompressed);
            bytes.push(0xac); // OP_CHECKSIG
            Ok(Some(bitcoin::ScriptBuf::from_bytes(bytes)))
        }
        _ => Ok(None),
    }
}

pub struct ScriptCompression(pub Vec<u8>);

impl ScriptCompression {
    const SPECIAL_SCRIPTS: usize = 6;

    pub fn serialize<W: std::io::Write>(&self, writer: &mut W) -> std::io::Result<()> {
        if let Some(compressed_script) = compress_script(&self.0) {
            writer.write_all(&compressed_script.0)?;
            return Ok(());
        }
        let size = self.0.len() + Self::SPECIAL_SCRIPTS;
        let mut data = Vec::new();
        VarInt::new(size as u64).consensus_encode(&mut data)?;
        writer.write_all(&data)?;
        writer.write_all(&self.0)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compress_amount() {
        let n = fastrand::u64(..MAX_MONEY);
        assert_eq!(n, decompress_amount(compress_amount(n)));
    }
}
