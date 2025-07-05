//! Subcoin Crypto Primitives.

pub mod muhash;

use chacha20::ChaCha20;
use chacha20::cipher::{KeyIvInit, StreamCipher, StreamCipherSeek};

// A function to generate the keystream from the test vectors
pub fn chacha20_block(key: &[u8; 32], nonce: &[u8; 12], counter: u32) -> Vec<u8> {
    let mut cipher = ChaCha20::new(key.into(), nonce.into());

    // Set the counter (as the starting position in the keystream)
    cipher.seek(counter as u64 * 64); // Each block is 64 bytes

    let mut keystream = vec![0u8; 64];
    cipher.apply_keystream(&mut keystream);

    keystream
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_literal::hex;

    // https://github.com/bitcoin/bitcoin/blob/6f9db1e/test/functional/test_framework/crypto/chacha20.py#L142
    #[test]
    fn test_chacha20() {
        // Test vectors from RFC7539/8439 consisting of 32 byte key, 12 byte nonce, block counter
        // and 64 byte output after applying `chacha20_block` function
        let chacha20_tests: Vec<([u8; 32], [u64; 2], u32, [u8; 64])> = vec![
            (
                hex!("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"),
                [0x09000000, 0x4a000000],
                1,
                hex!(
                    "10f1e7e4d13b5915500fdd1fa32071c4c7d1f4c733c068030422aa9ac3d46c4e\
                    d2826446079faa0914c2d705d98b02a2b5129cd1de164eb9cbd083e8a2503c4e"
                ),
            ),
            (
                hex!("0000000000000000000000000000000000000000000000000000000000000000"),
                [0, 0],
                0,
                hex!(
                    "76b8e0ada0f13d90405d6ae55386bd28bdd219b8a08ded1aa836efcc8b770dc7\
                    da41597c5157488d7724e03fb8d84a376a43b8f41518a11cc387b669b2ee6586"
                ),
            ),
            (
                hex!("0000000000000000000000000000000000000000000000000000000000000000"),
                [0, 0],
                1,
                hex!(
                    "9f07e7be5551387a98ba977c732d080dcb0f29a048e3656912c6533e32ee7aed\
                    29b721769ce64e43d57133b074d839d531ed1f28510afb45ace10a1f4b794d6f"
                ),
            ),
            (
                hex!("0000000000000000000000000000000000000000000000000000000000000001"),
                [0, 0],
                1,
                hex!(
                    "3aeb5224ecf849929b9d828db1ced4dd832025e8018b8160b82284f3c949aa5a\
                    8eca00bbb4a73bdad192b5c42f73f2fd4e273644c8b36125a64addeb006c13a0"
                ),
            ),
            (
                hex!("00ff000000000000000000000000000000000000000000000000000000000000"),
                [0, 0],
                2,
                hex!(
                    "72d54dfbf12ec44b362692df94137f328fea8da73990265ec1bbbea1ae9af0ca\
                    13b25aa26cb4a648cb9b9d1be65b2c0924a66c54d545ec1b7374f4872e99f096"
                ),
            ),
            (
                hex!("0000000000000000000000000000000000000000000000000000000000000000"),
                [0, 0x200000000000000],
                0,
                hex!(
                    "c2c64d378cd536374ae204b9ef933fcd1a8b2288b3dfa49672ab765b54ee27c7\
                    8a970e0e955c14f3a88e741b97c286f75f8fc299e8148362fa198a39531bed6d"
                ),
            ),
            (
                hex!("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"),
                [0, 0x4a000000],
                1,
                hex!(
                    "224f51f3401bd9e12fde276fb8631ded8c131f823d2c06e27e4fcaec9ef3cf78\
                    8a3b0aa372600a92b57974cded2b9334794cba40c63e34cdea212c4cf07d41b7"
                ),
            ),
            (
                hex!("0000000000000000000000000000000000000000000000000000000000000001"),
                [0, 0],
                0,
                hex!(
                    "4540f05a9f1fb296d7736e7b208e3c96eb4fe1834688d2604f450952ed432d41\
                    bbe2a0b6ea7566d2a5d1e7e20d42af2c53d792b1c43fea817e9ad275ae546963"
                ),
            ),
            (
                hex!("0000000000000000000000000000000000000000000000000000000000000000"),
                [0, 1],
                0,
                hex!(
                    "ef3fdfd6c61578fbf5cf35bd3dd33b8009631634d21e42ac33960bd138e50d32\
                    111e4caf237ee53ca8ad6426194a88545ddc497a0b466e7d6bbdb0041b2f586b"
                ),
            ),
            (
                hex!("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"),
                [0, 0x0706050403020100],
                0,
                hex!(
                    "f798a189f195e66982105ffb640bb7757f579da31602fc93ec01ac56f85ac3c1\
                    34a4547b733b46413042c9440049176905d3be59ea1c53f15916155c2be8241a"
                ),
            ),
        ];

        for (hex_key, nonce, counter, expected_output) in chacha20_tests {
            // Convert the key to a byte array
            let key = hex_key;

            let nonce_bytes = {
                let mut n = [0u8; 12];
                n[..4].copy_from_slice(&nonce[0].to_le_bytes()[..4]);
                n[4..].copy_from_slice(&nonce[1].to_le_bytes()[..8]);
                n
            };

            // Generate the keystream using the chacha20_block function
            let keystream = chacha20_block(&key, &nonce_bytes, counter);

            // Check that the generated keystream matches the expected output
            assert_eq!(expected_output, &keystream[..expected_output.len()]);
        }
    }
}
