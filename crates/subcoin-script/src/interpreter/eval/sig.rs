use crate::constants::{COMPRESSED_PUBKEY_SIZE, HALF_ORDER, VALIDATION_WEIGHT_PER_SIGOP_PASSED};
use crate::signature_checker::SignatureChecker;
use crate::{EcdsaSignature, SchnorrSignature, ScriptExecutionData, SigVersion, VerifyFlags};
use bitcoin::{PublicKey, Script, XOnlyPublicKey};
use num_bigint::Sign;

const SIGHASH_ALL: u8 = 0x01;
#[allow(unused)]
const SIGHASH_NONE: u8 = 0x02;
const SIGHASH_SINGLE: u8 = 0x03;
const SIGHASH_ANYONECANPAY: u8 = 0x80;

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum SignatureEncodingError {
    #[error("DER encoded signature is too short")]
    TooShort,
    #[error("DER encoded signature is too long")]
    TooLong,
    #[error("signature does not have the expected ASN.1 sequence ID")]
    InvalidSequenceId,
    #[error("signature length")]
    InvalidDataLength,
    #[error("R integer marker")]
    InvalidIntegerIdR,
    #[error("R length is zero")]
    ZeroLengthR,
    #[error("R is negative")]
    NegativeR,
    #[error("R value has too much padding")]
    TooMuchPaddingR,
    #[error("S integer marker")]
    InvalidIntegerIdS,
    #[error("S length is zero")]
    ZeroLengthS,
    #[error("S is negative")]
    NegativeS,
    #[error("S value has too much padding")]
    TooMuchPaddingS,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum CheckSigError {
    #[error("Signature found during find_and_delete in CONST_SCRIPTCODE mode")]
    FindAndDelete,
    #[error("Unsupported signature hash type")]
    UnsupportedSigHashType,
    #[error("discourage upgradable hash type")]
    DiscourageUpgradablePubkeyType,
    #[error("Unsupported public key type")]
    BadPubKey,
    #[error("public key type")]
    PubKeyType,
    #[error("ScriptVerifyWitness is set and the public key used in checksig/checkmultisig isn't serialized in a compressed format.")]
    WitnessPubKeyType,
    // Signatures are not empty on failed checksig or checkmultisig operations.
    #[error("signatures are not empty on failed checksig/checkmultisig operations.")]
    NullFail,
    #[error("Signature violates low-S requirement")]
    HighS,
    #[error("Exceeded tapscript validation weight")]
    TapscriptValidationWeight,
    #[error("invalid signature encoding: {0:?}")]
    Der(#[from] SignatureEncodingError),
    #[error("generating key from slice: {0:?}")]
    FromSlice(bitcoin::key::FromSliceError),
    #[error("schnorr signature error: {0:?}")]
    SigFromSlice(bitcoin::taproot::SigFromSliceError),
    #[error("invalid signature: {0:?}")]
    Signature(crate::signature_checker::SignatureError),
    #[error("secp256k1 error: {0:?}")]
    Secp256k1(bitcoin::secp256k1::Error),
    #[error("Failed to convert bytes to ECDSA signature: {0:?}")]
    Ecdsa(bitcoin::ecdsa::Error),
}

pub(super) fn handle_checksig(
    sig: &[u8],
    pubkey: &[u8],
    script: &Script,
    begincodehash: usize,
    exec_data: &mut ScriptExecutionData,
    flags: &VerifyFlags,
    checker: &mut impl SignatureChecker,
    sig_version: SigVersion,
) -> Result<bool, CheckSigError> {
    match sig_version {
        SigVersion::Base | SigVersion::WitnessV0 => eval_checksig_pre_tapscript(
            sig,
            pubkey,
            script,
            begincodehash,
            flags,
            checker,
            sig_version,
        ),
        SigVersion::Taproot => {
            eval_checksig_tapscript(sig, pubkey, flags, checker, sig_version, exec_data)
        }
        SigVersion::Tapscript => unreachable!("key path in Taproot has no script"),
    }
}

fn eval_checksig_pre_tapscript(
    sig: &[u8],
    pubkey: &[u8],
    script: &Script,
    begincodehash: usize,
    flags: &VerifyFlags,
    checker: &mut impl SignatureChecker,
    sig_version: SigVersion,
) -> Result<bool, CheckSigError> {
    let mut subscript = script.as_bytes()[begincodehash..].to_vec();

    // Drop the signature in pre-segwit scripts but not segwit scripts
    if matches!(sig_version, SigVersion::Base) {
        let found = find_and_delete(&mut subscript, sig);

        if found > 0 && flags.intersects(VerifyFlags::CONST_SCRIPTCODE) {
            return Err(CheckSigError::FindAndDelete);
        }
    }

    check_signature_encoding(sig, flags)?;
    check_pubkey_encoding(pubkey, flags, sig_version)?;

    let signature = EcdsaSignature::parse_der_lax(sig).map_err(CheckSigError::Ecdsa)?;
    let pubkey = PublicKey::from_slice(pubkey).map_err(CheckSigError::FromSlice)?;

    let script_code = Script::from_bytes(&subscript);

    let success = checker
        .check_ecdsa_signature(&signature, &pubkey, script_code, sig_version)
        .map_err(CheckSigError::Signature)?;

    if !success && flags.intersects(VerifyFlags::NULLFAIL) && !sig.is_empty() {
        return Err(CheckSigError::NullFail);
    }

    Ok(success)
}

pub(super) fn find_and_delete(script: &mut Vec<u8>, sig: &[u8]) -> usize {
    if sig.is_empty() || sig.len() > script.len() {
        return 0;
    }

    let mut found = 0;
    let mut result = Vec::with_capacity(script.len());
    let mut i = 0;

    while i <= script.len() - sig.len() {
        // Check for a match directly by comparing byte by byte
        if &script[i..i + sig.len()] == sig {
            found += 1;
            i += sig.len(); // Skip the matched signature
        } else {
            result.push(script[i]); // Keep the current byte if no match
            i += 1;
        }
    }

    // Append any remaining bytes after the last match
    result.extend_from_slice(&script[i..]);

    // Replace the original script with the modified one
    *script = result;

    found
}

pub(super) fn check_signature_encoding(
    sig: &[u8],
    flags: &VerifyFlags,
) -> Result<(), CheckSigError> {
    // Empty signature. Not strictly DER encoded, but allowed to provide a
    // compact way to provide an invalid signature for use with CHECK(MULTI)SIG
    if sig.is_empty() {
        return Ok(());
    }

    if flags.intersects(VerifyFlags::DERSIG | VerifyFlags::LOW_S | VerifyFlags::STRICTENC) {
        is_valid_signature_encoding(sig).map_err(CheckSigError::Der)?;
    }

    if flags.intersects(VerifyFlags::LOW_S) {
        is_low_der_signature(sig)?;
    }

    if flags.intersects(VerifyFlags::STRICTENC) && !is_defined_hashtype_signature(sig) {
        return Err(CheckSigError::UnsupportedSigHashType);
    }

    Ok(())
}

struct EncodedS {
    /// S offset
    offset: usize,
    /// S length.
    length: usize,
}

// here is how to encode signatures correctly in DER format.
// 0x30 [total-length] 0x02 [R-length] [R] 0x02 [S-length] [S] [sighash-type]
//
// https://github.com/bitcoin/bips/blob/master/bip-0062.mediawiki#der-encoding
fn is_valid_signature_encoding(sig: &[u8]) -> Result<EncodedS, SignatureEncodingError> {
    // Minimum and maximum size constraints
    if sig.len() < 9 {
        return Err(SignatureEncodingError::TooShort);
    }

    if sig.len() > 73 {
        return Err(SignatureEncodingError::TooLong);
    }

    // A signature is of type 0x30 (compound)
    if sig[0] != 0x30 {
        return Err(SignatureEncodingError::InvalidSequenceId);
    }

    // Make sure the length covers the entire signature
    if sig[1] as usize != sig.len() - 3 {
        return Err(SignatureEncodingError::InvalidDataLength);
    }

    // Extract the length of the R element
    let len_r = sig[3] as usize;

    // Make sure the length of the S element is still inside the signature
    if 5 + len_r >= sig.len() {
        return Err(SignatureEncodingError::InvalidDataLength);
    }

    // Extract the length of the S element
    let len_s = sig[5 + len_r] as usize;

    // Verify that the length of the signature matches the sum of the length of the elements
    if len_r + len_s + 7 != sig.len() {
        return Err(SignatureEncodingError::InvalidDataLength);
    }

    // Check whether the R element is an integer
    if sig[2] != 0x02 {
        return Err(SignatureEncodingError::InvalidIntegerIdR);
    }

    // Zero-length integers are not allowed for R
    if len_r == 0 {
        return Err(SignatureEncodingError::ZeroLengthR);
    }

    // Negative numbers are not allowed for R
    if sig[4] & 0x80 != 0 {
        return Err(SignatureEncodingError::NegativeR);
    }

    // Null bytes at the start of R are not allowed, unless R would otherwise be interpreted as a negative number
    if len_r > 1 && sig[4] == 0x00 && sig[5] & 0x80 == 0 {
        return Err(SignatureEncodingError::TooMuchPaddingR);
    }

    // Check whether the S element is an integer
    if sig[len_r + 4] != 0x02 {
        return Err(SignatureEncodingError::InvalidIntegerIdS);
    }

    // Zero-length integers are not allowed for S
    if len_s == 0 {
        return Err(SignatureEncodingError::ZeroLengthS);
    }

    // Negative numbers are not allowed for S
    if sig[len_r + 6] & 0x80 != 0 {
        return Err(SignatureEncodingError::NegativeS);
    }

    // Null bytes at the start of S are not allowed, unless S would otherwise be interpreted as a negative number
    if len_s > 1 && sig[len_r + 6] == 0x00 && sig[len_r + 7] & 0x80 == 0 {
        return Err(SignatureEncodingError::TooMuchPaddingS);
    }

    Ok(EncodedS {
        offset: len_r + 6,
        length: len_s,
    })
}

fn is_low_der_signature(sig: &[u8]) -> Result<(), CheckSigError> {
    let encoded_s = is_valid_signature_encoding(sig).map_err(CheckSigError::Der)?;

    let s_bytes = &sig[encoded_s.offset..encoded_s.offset + encoded_s.length];

    // Verify the S value is <= half the order of the curve.  This check is done
    // because when it is higher, the complement modulo the order can be used
    // instead which is a shorter encoding by 1 byte.  Further, without
    // enforcing this, it is possible to replace a signature in a valid
    // transaction with the complement while still being a valid signature that
    // verifies.  This would result in changing the transaction hash and thus is
    // a source of malleability.
    let s_value = num_bigint::BigInt::from_bytes_be(Sign::Plus, &s_bytes);

    if s_value > *HALF_ORDER {
        // signature is not canonical due to unnecessarily high S value
        return Err(CheckSigError::HighS);
    }

    Ok(())
}

fn is_defined_hashtype_signature(sig: &[u8]) -> bool {
    let Some(last_bit) = sig.last().copied() else {
        return false;
    };

    // Extract the hash type byte from the signature
    let n_hash_type = last_bit & !SIGHASH_ANYONECANPAY;

    // Check if the hash type is within the valid range
    n_hash_type >= SIGHASH_ALL && n_hash_type <= SIGHASH_SINGLE
}

// Checks whether or not the passed public key adheres to
// the strict encoding requirements if enabled.
pub(super) fn check_pubkey_encoding(
    pubkey: &[u8],
    flags: &VerifyFlags,
    sig_version: SigVersion,
) -> Result<(), CheckSigError> {
    if flags.intersects(VerifyFlags::STRICTENC) && !is_public_key(pubkey) {
        return Err(CheckSigError::BadPubKey);
    }

    if flags.intersects(VerifyFlags::WITNESS_PUBKEYTYPE)
        && matches!(sig_version, SigVersion::WitnessV0)
        && !is_compressed_pubkey(pubkey)
    {
        return Err(CheckSigError::WitnessPubKeyType);
    }

    Ok(())
}

fn is_public_key(v: &[u8]) -> bool {
    match v.len() {
        33 if v[0] == 2 || v[0] == 3 => true, // Compressed
        65 if v[0] == 4 => true,              // Uncompressed
        _ => false,
    }
}

fn is_compressed_pubkey(pubkey: &[u8]) -> bool {
    // Check if the public key is the correct size for a compressed key
    if pubkey.len() != COMPRESSED_PUBKEY_SIZE {
        return false;
    }

    // Check if the public key has a valid prefix for a compressed key
    matches!(pubkey[0], 0x02 | 0x03)
}

fn eval_checksig_tapscript(
    sig: &[u8],
    pubkey: &[u8],
    flags: &VerifyFlags,
    checker: &mut impl SignatureChecker,
    sig_version: SigVersion,
    exec_data: &mut ScriptExecutionData,
) -> Result<bool, CheckSigError> {
    // The following validation sequence is consensus critical. Please note how --
    // upgradable public key versions precede other rules;
    // the script execution fails when using empty signature with invalid public key;
    // the script execution fails when using non-empty invalid signature.
    let success = !sig.is_empty();

    if success {
        // Implement the sigops/witnesssize ratio test.
        // Passing with an upgradable public key version is also counted.
        assert!(exec_data.validation_weight_left_init);
        exec_data.validation_weight_left = exec_data
            .validation_weight_left
            .checked_sub(VALIDATION_WEIGHT_PER_SIGOP_PASSED)
            .ok_or(CheckSigError::TapscriptValidationWeight)?;
    }

    match pubkey.len() {
        0 => return Err(CheckSigError::PubKeyType),
        32 => {
            if success {
                let sig = SchnorrSignature::from_slice(sig).map_err(CheckSigError::SigFromSlice)?;
                let pubkey =
                    XOnlyPublicKey::from_slice(pubkey).map_err(CheckSigError::Secp256k1)?;

                if !checker
                    .check_schnorr_signature(&sig, &pubkey, sig_version, exec_data)
                    .map_err(CheckSigError::Signature)?
                {
                    return Ok(false);
                }
            }
        }
        _ => {
            // New public key version softforks should be defined before this `else` block.
            // Generally, the new code should not do anything but failing the script execution. To avoid
            // consensus bugs, it should not modify any existing values (including `success`).
            if flags.intersects(VerifyFlags::DISCOURAGE_UPGRADABLE_PUBKEYTYPE) {
                return Err(CheckSigError::DiscourageUpgradablePubkeyType);
            }
        }
    }

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_public_key() {
        assert!(!is_public_key(&[]));
        assert!(!is_public_key(&[1]));
        assert!(is_public_key(&hex::decode("0495dfb90f202c7d016ef42c65bc010cd26bb8237b06253cc4d12175097bef767ed6b1fcb3caf1ed57c98d92e6cb70278721b952e29a335134857acd4c199b9d2f").unwrap()));
        assert!(is_public_key(&[2; 33]));
        assert!(is_public_key(&[3; 33]));
        assert!(!is_public_key(&[4; 33]));
    }

    // https://github.com/btcsuite/btcd/blob/ff2e03e11233fa25c01cf4acbf76501fc008b31f/txscript/engine_test.go#L201
    #[test]
    fn test_check_pubkey_encoding() {
        let tests = [
            // uncompressed ok
            (
                [
                    "0411db93e1dcdb8a016b49840f8c53bc1eb68",
                    "a382e97b1482ecad7b148a6909a5cb2e0eaddfb84ccf",
                    "9744464f82e160bfa9b8b64f9d4c03f999b8643f656b",
                    "412a3",
                ]
                .concat(),
                Ok(()),
            ),
            // compressed ok
            (
                [
                    "02ce0b14fb842b1ba549fdd675c98075f12e9",
                    "c510f8ef52bd021a9a1f4809d3b4d",
                ]
                .concat(),
                Ok(()),
            ),
            // compressed ok
            (
                [
                    "032689c7c2dab13309fb143e0e8fe39634252",
                    "1887e976690b6b47f5b2a4b7d448e",
                ]
                .concat(),
                Ok(()),
            ),
            // empty
            ("".to_string(), Err(CheckSigError::BadPubKey)),
            // hybrid
            (
                [
                    "0679be667ef9dcbbac55a06295ce870b07029",
                    "bfcdb2dce28d959f2815b16f81798483ada7726a3c46",
                    "55da4fbfc0e1108a8fd17b448a68554199c47d08ffb1",
                    "0d4b8",
                ]
                .concat(),
                Err(CheckSigError::BadPubKey),
            ),
        ];

        let flags = VerifyFlags::STRICTENC;

        for (hex_str, expected_result) in tests {
            let key = hex::decode(hex_str).expect("Invalid hex string");
            let result = check_pubkey_encoding(&key, &flags, SigVersion::Base);
            assert_eq!(result, expected_result)
        }
    }

    // https://github.com/btcsuite/btcd/blob/ff2e03e11233fa25c01cf4acbf76501fc008b31f/txscript/engine_test.go#L261
    #[test]
    fn test_check_signature_encoding() {
        struct TestCase {
            name: &'static str,
            sig: String,
            is_valid: bool,
        }

        let test_cases = [
            TestCase {
                name: "valid signature",
                sig: [
                    "304402204e45e16932b8af514961a1d3a1a25",
                    "fdf3f4f7732e9d624c6c61548ab5fb8cd41022018152",
                    "2ec8eca07de4860a4acdd12909d831cc56cbbac46220",
                    "82221a8768d1d09",
                ]
                .concat(),
                is_valid: true,
            },
            TestCase {
                name: "empty.",
                sig: "".to_string(),
                is_valid: true, // FIXME: epmty signature is allowed or not?
            },
            TestCase {
                name: "bad magic",
                sig: [
                    "314402204e45e16932b8af514961a1d3a1a25",
                    "fdf3f4f7732e9d624c6c61548ab5fb8cd41022018152",
                    "2ec8eca07de4860a4acdd12909d831cc56cbbac46220",
                    "82221a8768d1d09",
                ]
                .concat(),
                is_valid: false,
            },
            TestCase {
                name: "bad 1st int marker magic",
                sig: [
                    "304403204e45e16932b8af514961a1d3a1a25",
                    "fdf3f4f7732e9d624c6c61548ab5fb8cd41022018152",
                    "2ec8eca07de4860a4acdd12909d831cc56cbbac46220",
                    "82221a8768d1d09",
                ]
                .concat(),
                is_valid: false,
            },
            TestCase {
                name: "bad 2nd int marker",
                sig: [
                    "304402204e45e16932b8af514961a1d3a1a25",
                    "fdf3f4f7732e9d624c6c61548ab5fb8cd41032018152",
                    "2ec8eca07de4860a4acdd12909d831cc56cbbac46220",
                    "82221a8768d1d09",
                ]
                .concat(),
                is_valid: false,
            },
            TestCase {
                name: "short len",
                sig: [
                    "304302204e45e16932b8af514961a1d3a1a25",
                    "fdf3f4f7732e9d624c6c61548ab5fb8cd41022018152",
                    "2ec8eca07de4860a4acdd12909d831cc56cbbac46220",
                    "82221a8768d1d09",
                ]
                .concat(),
                is_valid: false,
            },
            TestCase {
                name: "long len",
                sig: [
                    "304502204e45e16932b8af514961a1d3a1a25",
                    "fdf3f4f7732e9d624c6c61548ab5fb8cd41022018152",
                    "2ec8eca07de4860a4acdd12909d831cc56cbbac46220",
                    "82221a8768d1d09",
                ]
                .concat(),
                is_valid: false,
            },
            TestCase {
                name: "long X",
                sig: [
                    "304402424e45e16932b8af514961a1d3a1a25",
                    "fdf3f4f7732e9d624c6c61548ab5fb8cd41022018152",
                    "2ec8eca07de4860a4acdd12909d831cc56cbbac46220",
                    "82221a8768d1d09",
                ]
                .concat(),
                is_valid: false,
            },
            TestCase {
                name: "long Y",
                sig: [
                    "304402204e45e16932b8af514961a1d3a1a25",
                    "fdf3f4f7732e9d624c6c61548ab5fb8cd41022118152",
                    "2ec8eca07de4860a4acdd12909d831cc56cbbac46220",
                    "82221a8768d1d09",
                ]
                .concat(),
                is_valid: false,
            },
            TestCase {
                name: "short Y",
                sig: [
                    "304402204e45e16932b8af514961a1d3a1a25",
                    "fdf3f4f7732e9d624c6c61548ab5fb8cd41021918152",
                    "2ec8eca07de4860a4acdd12909d831cc56cbbac46220",
                    "82221a8768d1d09",
                ]
                .concat(),
                is_valid: false,
            },
            TestCase {
                name: "trailing crap",
                sig: [
                    "304402204e45e16932b8af514961a1d3a1a25",
                    "fdf3f4f7732e9d624c6c61548ab5fb8cd41022018152",
                    "2ec8eca07de4860a4acdd12909d831cc56cbbac46220",
                    "82221a8768d1d0901",
                ]
                .concat(),
                is_valid: false,
            },
            TestCase {
                name: "X == N ",
                sig: [
                    "30440220fffffffffffffffffffffffffffff",
                    "ffebaaedce6af48a03bbfd25e8cd0364141022018152",
                    "2ec8eca07de4860a4acdd12909d831cc56cbbac46220",
                    "82221a8768d1d09",
                ]
                .concat(),
                is_valid: false,
            },
            TestCase {
                name: "X == N ",
                sig: [
                    "30440220fffffffffffffffffffffffffffff",
                    "ffebaaedce6af48a03bbfd25e8cd0364142022018152",
                    "2ec8eca07de4860a4acdd12909d831cc56cbbac46220",
                    "82221a8768d1d09",
                ]
                .concat(),
                is_valid: false,
            },
            TestCase {
                name: "Y == N",
                sig: [
                    "304402204e45e16932b8af514961a1d3a1a25",
                    "fdf3f4f7732e9d624c6c61548ab5fb8cd410220fffff",
                    "ffffffffffffffffffffffffffebaaedce6af48a03bb",
                    "fd25e8cd0364141",
                ]
                .concat(),
                is_valid: false,
            },
            TestCase {
                name: "Y > N",
                sig: [
                    "304402204e45e16932b8af514961a1d3a1a25",
                    "fdf3f4f7732e9d624c6c61548ab5fb8cd410220fffff",
                    "ffffffffffffffffffffffffffebaaedce6af48a03bb",
                    "fd25e8cd0364142",
                ]
                .concat(),
                is_valid: false,
            },
            TestCase {
                name: "0 len X",
                sig: [
                    "302402000220181522ec8eca07de4860a4acd",
                    "d12909d831cc56cbbac4622082221a8768d1d09",
                ]
                .concat(),
                is_valid: false,
            },
            TestCase {
                name: "0 len Y",
                sig: [
                    "302402204e45e16932b8af514961a1d3a1a25",
                    "fdf3f4f7732e9d624c6c61548ab5fb8cd410200",
                ]
                .concat(),
                is_valid: false,
            },
            TestCase {
                name: "extra R padding",
                sig: [
                    "30450221004e45e16932b8af514961a1d3a1a",
                    "25fdf3f4f7732e9d624c6c61548ab5fb8cd410220181",
                    "522ec8eca07de4860a4acdd12909d831cc56cbbac462",
                    "2082221a8768d1d09",
                ]
                .concat(),
                is_valid: false,
            },
            TestCase {
                name: "extra S padding",
                sig: [
                    "304502204e45e16932b8af514961a1d3a1a25",
                    "fdf3f4f7732e9d624c6c61548ab5fb8cd41022100181",
                    "522ec8eca07de4860a4acdd12909d831cc56cbbac462",
                    "2082221a8768d1d09",
                ]
                .concat(),
                is_valid: false,
            },
        ];

        let flags = VerifyFlags::STRICTENC;

        for TestCase {
            name,
            sig,
            is_valid,
        } in test_cases
        {
            println!("==== name: {name}");
            // The argument `sig` in checkSignatureEncoding(sig []byte) in btcd is the signature
            // excluding the last sighash bit, but our `check_pubkey_encoding` is translated from
            // the cpp version which passes in the full sig, thus we push the SigHash::Base(0x01)
            // manually for simplicity.
            let full_sig = if sig.is_empty() {
                sig
            } else {
                format!("{sig}01")
            };
            let full_sig = hex::decode(full_sig).expect("Invalid hex string");
            let check_result = check_signature_encoding(&full_sig, &flags);
            assert_eq!(is_valid, check_result.is_ok());
        }
    }
}
