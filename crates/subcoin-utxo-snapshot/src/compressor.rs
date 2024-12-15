use crate::serialize::VarInt;
use std::vec::Vec;

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
        let d = (n % 10) as u64;
        assert!(d >= 1 && d <= 9);
        n /= 10;
        1 + (n * 9 + d - 1) * 10 + e as u64
    } else {
        1 + (n - 1) * 10 + 9
    }
}

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
        x * 10 + d as u64
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
        let mut hash = [0u8; 20];
        hash.copy_from_slice(&script[3..23]);
        Some(hash)
    } else {
        None
    }
}

fn to_script_id(script: &[u8]) -> Option<[u8; 20]> {
    if script.len() == 23 && script[0] == OP_HASH160 && script[1] == 20 && script[22] == OP_EQUAL {
        let mut hash = [0u8; 20];
        hash.copy_from_slice(&script[2..22]);
        Some(hash)
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
        let mut pubkey = [0u8; 33];
        pubkey.copy_from_slice(&script[1..34]);
        Some(PublicKey::Compressed(pubkey))
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
            let mut pubkey = [0u8; 65];
            pubkey.copy_from_slice(&script[1..66]);
            Some(PublicKey::Uncompressed(pubkey))
        } else {
            None
        }
    } else {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompressedScript(Vec<u8>);

fn compress_script(script: &[u8]) -> Option<CompressedScript> {
    if let Some(hash) = to_key_id(script) {
        let mut out = Vec::with_capacity(21);
        out.push(0x00);
        out.extend(hash);
        Some(CompressedScript(out))
    } else if let Some(hash) = to_script_id(script) {
        let mut out = Vec::with_capacity(21);
        out.push(0x01);
        out.extend(hash);
        Some(CompressedScript(out))
    } else if let Some(public_key) = to_pub_key(script) {
        let mut out = Vec::with_capacity(33);

        match public_key {
            PublicKey::Compressed(compressed) => {
                out.push(compressed[0]);
                out.extend_from_slice(&compressed[1..33]);
            }
            PublicKey::Uncompressed(uncompressed) => {
                out.push(0x04 | (uncompressed[64] & 0x01));
                out.extend_from_slice(&uncompressed[1..33]);
            }
        }

        Some(CompressedScript(out))
    } else {
        None
    }
}

pub struct ScriptCompression(pub Vec<u8>);

impl ScriptCompression {
    const SPECIAL_SCRIPTS: usize = 6;

    pub fn serialize<W: std::io::Write>(&self, writer: &mut W) -> std::io::Result<()> {
        if let Some(compressed_script) = compress_script(&self.0) {
            writer.write_all(&compressed_script.0)?;
        } else {
            let size = self.0.len() + Self::SPECIAL_SCRIPTS;
            VarInt(size as u64).serialize(writer)?;
            writer.write_all(&self.0)?;
        }
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
