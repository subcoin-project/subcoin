use crate::compressor::compress_script;
use bitcoin::consensus::encode::Error;
use bitcoin::consensus::{Decodable, Encodable};
use bitcoin::hashes::Hash;
use bitcoin::script::{Builder, ScriptBuf};
use bitcoin::{PubkeyHash, PublicKey, ScriptHash, opcodes};
use txoutset::var_int::VarInt;

const NUM_SPECIAL_SCRIPTS: usize = 6;
const MAX_SCRIPT_SIZE: usize = 10_000;

/// Wrapper to enable script decompression
#[derive(Debug)]
pub struct ScriptCompression(ScriptBuf);

impl Encodable for ScriptCompression {
    fn consensus_encode<W: bitcoin::io::Write + ?Sized>(
        &self,
        writer: &mut W,
    ) -> Result<usize, bitcoin::io::Error> {
        if let Some(compressed_script) = compress_script(self.0.as_bytes()) {
            let len = compressed_script.0.len();
            writer.write_all(&compressed_script.0)?;
            return Ok(len);
        }

        let size = self.0.len() + NUM_SPECIAL_SCRIPTS;
        let mut data = Vec::new();
        let mut len = VarInt::new(size as u64).consensus_encode(&mut data)?;
        writer.write_all(&data)?;

        len += self.0.len();
        writer.write_all(self.0.as_bytes())?;
        Ok(len)
    }
}

impl Decodable for ScriptCompression {
    fn consensus_decode<R: bitcoin::io::BufRead + ?Sized>(reader: &mut R) -> Result<Self, Error> {
        let mut size = u64::from(VarInt::consensus_decode(reader)?) as usize;

        match size {
            0x00 => {
                // P2PKH
                let mut bytes = [0; 20];
                reader.read_exact(&mut bytes)?;
                let pubkey_hash = PubkeyHash::from_slice(&bytes)
                    .map_err(|_| Error::ParseFailed("Failed to parse Hash160"))?;
                Ok(Self(ScriptBuf::new_p2pkh(&pubkey_hash)))
            }
            0x01 => {
                // P2SH
                let mut bytes = [0; 20];
                reader.read_exact(&mut bytes)?;
                let script_hash = ScriptHash::from_slice(&bytes)
                    .map_err(|_| Error::ParseFailed("Failed to parse Hash160"))?;
                Ok(Self(ScriptBuf::new_p2sh(&script_hash)))
            }
            0x02 | 0x03 => {
                // P2PK (compressed)
                let mut bytes = [0; 32];
                reader.read_exact(&mut bytes)?;

                let mut script_bytes = Vec::with_capacity(35);
                script_bytes.push(opcodes::all::OP_PUSHBYTES_33.to_u8());
                script_bytes.push(size as u8);
                script_bytes.extend_from_slice(&bytes);
                script_bytes.push(opcodes::all::OP_CHECKSIG.to_u8());

                Ok(Self(ScriptBuf::from(script_bytes)))
            }
            0x04 | 0x05 => {
                // P2PK (uncompressed)
                let mut bytes = [0; 32];
                reader.read_exact(&mut bytes)?;

                let mut compressed_pubkey_bytes = Vec::with_capacity(33);
                compressed_pubkey_bytes.push((size - 2) as u8);
                compressed_pubkey_bytes.extend_from_slice(&bytes);

                let compressed_pubkey = PublicKey::from_slice(&compressed_pubkey_bytes)
                    .map_err(|_| Error::ParseFailed("Failed to parse PublicKey"))?;
                let inner_uncompressed = compressed_pubkey.inner.serialize_uncompressed();

                let mut script_bytes = Vec::with_capacity(67);
                script_bytes.push(opcodes::all::OP_PUSHBYTES_65.to_u8());
                script_bytes.extend_from_slice(&inner_uncompressed);
                script_bytes.push(opcodes::all::OP_CHECKSIG.to_u8());

                Ok(Self(ScriptBuf::from(script_bytes)))
            }
            _ => {
                size -= NUM_SPECIAL_SCRIPTS;
                let mut bytes = Vec::with_capacity(size);
                bytes.resize_with(size, || 0);
                if size > MAX_SCRIPT_SIZE {
                    reader.read_exact(&mut bytes)?;
                    let script = Builder::new()
                        .push_opcode(opcodes::all::OP_RETURN)
                        .into_script();
                    Ok(Self(script))
                } else {
                    reader.read_exact(&mut bytes)?;
                    Ok(Self(ScriptBuf::from_bytes(bytes)))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_script_compression() {
        let line = "7e27944eacef755d2fa83b6559f480941aa4f0e5f3f0623fc3d0de9cd28c0100:1,false,413198,7800,5121030b3810fd20fd3771517b2b8847d225791035ea06768e17c733a5756b6005bf55210222b6e887bb4d4bca08f97348e6b8561e6d11e0ed96dec0584b34d709078cd4a54104289699814d1c9ef35ae45cfb41116501c15b0141430a481226aa19bcb8806c7223802d24f2638d8ce14378137dd52114d1d965e2969b5b3ac011c25e2803eb5753ae";

        let utxo = crate::tests::parse_csv_entry(line);
        let script = utxo.coin.script_pubkey;

        let script_compression = ScriptCompression(ScriptBuf::from_bytes(script.clone()));

        let mut encoded = Vec::new();
        script_compression.consensus_encode(&mut encoded).unwrap();

        let decoded = ScriptCompression::consensus_decode(&mut encoded.as_slice()).unwrap();

        assert_eq!(decoded.0, script_compression.0);
    }
}
