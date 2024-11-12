//! https://github.com/bitcoin/bitcoin/blob/6f9db1ebcab4064065ccd787161bf2b87e03cc1f/test/functional/test_framework/crypto/muhash.py#L4

use crate::chacha20_block;
use num_bigint::{BigUint, ToBigUint};
use num_traits::One;
use sha2::{Digest, Sha256};
use std::fmt::Write;

// Function to hash a 32-byte array into a 3072-bit number using 6 ChaCha20 operations
fn data_to_num3072(data: &[u8; 32]) -> BigUint {
    let mut bytes384 = Vec::new();
    for counter in 0..6 {
        bytes384.extend(chacha20_block(data, &[0u8; 12], counter));
    }
    BigUint::from_bytes_le(&bytes384)
}

/// A class representing MuHash sets.
///
/// https://github.com/bitcoin/bitcoin/blob/6f9db1e/src/crypto/muhash.h#L61
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct MuHash3072 {
    numerator: BigUint,
    denominator: BigUint,
    modulus: BigUint,
}

impl Default for MuHash3072 {
    fn default() -> Self {
        Self::new()
    }
}

impl MuHash3072 {
    // Create a new [`MuHash3072`] with the appropriate modulus.
    pub fn new() -> Self {
        let modulus = (BigUint::one() << 3072) - 1103717u32.to_biguint().unwrap();
        Self {
            numerator: BigUint::one(),
            denominator: BigUint::one(),
            modulus,
        }
    }

    // Insert a byte array into the set
    pub fn insert(&mut self, data: &[u8]) {
        let data_hash = Sha256::digest(data);
        let num3072 = data_to_num3072(&data_hash.into());
        self.numerator *= num3072;
        self.numerator %= &self.modulus;
    }

    // Remove a byte array from the set
    pub fn remove(&mut self, data: &[u8]) {
        let data_hash = Sha256::digest(data);
        let num3072 = data_to_num3072(&data_hash.into());
        self.denominator *= num3072;
        self.denominator %= &self.modulus;
    }

    // Compute the final digest
    pub fn digest(&self) -> Vec<u8> {
        let denominator_inv = self
            .denominator
            .modpow(&(self.modulus.clone() - 2u32), &self.modulus);
        let val = (&self.numerator * denominator_inv) % &self.modulus;
        let mut bytes384 = val.to_bytes_le();
        bytes384.resize(384, 0); // Ensure it is exactly 384 bytes
        Sha256::digest(&bytes384).to_vec()
    }

    /// Returns the value of `muhash` in Bitcoin Core's dumptxoutset output.
    pub fn txoutset_muhash(&self) -> String {
        let finalized = self.digest();

        finalized.iter().rev().fold(String::new(), |mut output, b| {
            let _ = write!(output, "{b:02x}");
            output
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // https://github.com/bitcoin/bitcoin/blob/6f9db1e/test/functional/test_framework/crypto/muhash.py#L48
    #[test]
    fn test_muhash() {
        let mut muhash = MuHash3072::new();
        muhash.insert(&[0x00; 32]);

        // Insert 32 bytes, first byte 0x01, rest 0x00
        let mut data = Vec::with_capacity(32);
        data.push(0x01);
        data.extend_from_slice(&[0x00; 31]);
        muhash.insert(&data);

        // Remove 32 bytes, first byte 0x02, rest 0x00
        let mut data = Vec::with_capacity(32);
        data.push(0x02);
        data.extend_from_slice(&[0x00; 31]);
        muhash.remove(&data);

        let finalized = muhash.digest();
        assert_eq!(
            finalized
                .iter()
                .rev()
                .map(|b| format!("{:02x}", b))
                .collect::<String>(),
            "10d312b100cbd32ada024a6646e40d3482fcff103668d2625f10002a607d5863"
        );
    }
}
