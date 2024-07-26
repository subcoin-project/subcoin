use crate::flags::VerificationFlags;
use crate::Error;
use bitcoin::Opcode;
use bitcoin::Script;

/// Checks transaction signature
pub trait SignatureChecker {
    fn verify_signature(&self, signature: &Signature, public: &Public, hash: &Message) -> bool;

    fn check_signature(
        &self,
        signature: &Signature,
        public: &Public,
        script_code: &Script,
        sighashtype: u32,
        version: SignatureVersion,
    ) -> bool;

    fn check_lock_time(&self, lock_time: Num) -> bool;

    fn check_sequence(&self, sequence: Num) -> bool;
}

/// Helper function.
pub fn check_signature(
    checker: &dyn SignatureChecker,
    script_sig: &Vec<u8>,
    public: &Vec<u8>,
    script_code: &Script,
    version: SignatureVersion,
) -> bool {
    let public = match Public::from_slice(&public) {
        Ok(public) => public,
        _ => return false,
    };

    if let Some((hash_type, sig)) = script_sig.split_last() {
        checker.check_signature(
            &sig.into(),
            &public,
            script_code,
            *hash_type as u32,
            version,
        )
    } else {
        return false;
    }
}

/// Helper function.
pub fn verify_signature(
    checker: &dyn SignatureChecker,
    signature: Vec<u8>,
    public: Vec<u8>,
    message: Message,
) -> bool {
    let public = match Public::from_slice(&public) {
        Ok(public) => public,
        _ => return false,
    };

    if signature.is_empty() {
        return false;
    }

    checker.verify_signature(&signature.into(), &public, &message.into())
}

pub fn is_public_key(v: &[u8]) -> bool {
    match v.len() {
        33 if v[0] == 2 || v[0] == 3 => true,
        65 if v[0] == 4 => true,
        _ => false,
    }
}

/// A canonical signature exists of: <30> <total len> <02> <len R> <R> <02> <len S> <S> <hashtype>
/// Where R and S are not negative (their first byte has its highest bit not set), and not
/// excessively padded (do not start with a 0 byte, unless an otherwise negative number follows,
/// in which case a single 0 byte is necessary and even required).
///
/// See https://bitcointalk.org/index.php?topic=8392.msg127623#msg127623
///
/// This function is consensus-critical since BIP66.
pub fn is_valid_signature_encoding(sig: &[u8]) -> bool {
    // Format: 0x30 [total-length] 0x02 [R-length] [R] 0x02 [S-length] [S] [sighash]
    // * total-length: 1-byte length descriptor of everything that follows,
    //   excluding the sighash byte.
    // * R-length: 1-byte length descriptor of the R value that follows.
    // * R: arbitrary-length big-endian encoded R value. It must use the shortest
    //   possible encoding for a positive integers (which means no null bytes at
    //   the start, except a single one when the next byte has its highest bit set).
    // * S-length: 1-byte length descriptor of the S value that follows.
    // * S: arbitrary-length big-endian encoded S value. The same rules apply.
    // * sighash: 1-byte value indicating what data is hashed (not part of the DER
    //   signature)

    // Minimum and maximum size constraints
    if sig.len() < 9 || sig.len() > 73 {
        return false;
    }

    // A signature is of type 0x30 (compound)
    if sig[0] != 0x30 {
        return false;
    }

    // Make sure the length covers the entire signature.
    if sig[1] as usize != sig.len() - 3 {
        return false;
    }

    // Extract the length of the R element.
    let len_r = sig[3] as usize;

    // Make sure the length of the S element is still inside the signature.
    if len_r + 5 >= sig.len() {
        return false;
    }

    // Extract the length of the S element.
    let len_s = sig[len_r + 5] as usize;

    // Verify that the length of the signature matches the sum of the length
    if len_r + len_s + 7 != sig.len() {
        return false;
    }

    // Check whether the R element is an integer.
    if sig[2] != 2 {
        return false;
    }

    // Zero-length integers are not allowed for R.
    if len_r == 0 {
        return false;
    }

    // Negative numbers are not allowed for R.
    if (sig[4] & 0x80) != 0 {
        return false;
    }

    // Null bytes at the start of R are not allowed, unless R would
    // otherwise be interpreted as a negative number.
    if len_r > 1 && sig[4] == 0 && (sig[5] & 0x80) == 0 {
        return false;
    }

    // Check whether the S element is an integer.
    if sig[len_r + 4] != 2 {
        return false;
    }

    // Zero-length integers are not allowed for S.
    if len_s == 0 {
        return false;
    }

    // Negative numbers are not allowed for S.
    if (sig[len_r + 6] & 0x80) != 0 {
        return false;
    }

    // Null bytes at the start of S are not allowed, unless S would otherwise be
    // interpreted as a negative number.
    if len_s > 1 && (sig[len_r + 6] == 0) && (sig[len_r + 7] & 0x80) == 0 {
        return false;
    }

    true
}

fn is_low_der_signature(sig: &[u8]) -> Result<(), Error> {
    if !is_valid_signature_encoding(sig) {
        return Err(Error::SignatureDer);
    }

    let signature: Signature = sig.into();
    if !signature.check_low_s() {
        return Err(Error::SignatureHighS);
    }

    Ok(())
}

fn is_defined_hashtype_signature(version: SignatureVersion, sig: &[u8]) -> bool {
    if sig.is_empty() {
        return false;
    }

    Sighash::is_defined(version, sig[sig.len() - 1] as u32)
}

fn parse_hash_type(version: SignatureVersion, sig: &[u8]) -> Sighash {
    Sighash::from_u32(
        version,
        if sig.is_empty() {
            0
        } else {
            sig[sig.len() - 1] as u32
        },
    )
}

fn check_signature_encoding(
    sig: &[u8],
    flags: &VerificationFlags,
    version: SignatureVersion,
) -> Result<(), Error> {
    // Empty signature. Not strictly DER encoded, but allowed to provide a
    // compact way to provide an invalid signature for use with CHECK(MULTI)SIG

    if sig.is_empty() {
        return Ok(());
    }

    if (flags.verify_dersig || flags.verify_low_s || flags.verify_strictenc)
        && !is_valid_signature_encoding(sig)
    {
        return Err(Error::SignatureDer);
    }

    if flags.verify_low_s {
        is_low_der_signature(sig)?;
    }

    if flags.verify_strictenc && !is_defined_hashtype_signature(version, sig) {
        return Err(Error::SignatureHashtype);
    }

    // verify_strictenc is currently enabled for BitcoinCash only
    if flags.verify_strictenc {
        let uses_fork_id = parse_hash_type(version, sig).fork_id;
        let enabled_fork_id = version == SignatureVersion::ForkId;
        if uses_fork_id && !enabled_fork_id {
            return Err(Error::SignatureIllegalForkId);
        } else if !uses_fork_id && enabled_fork_id {
            return Err(Error::SignatureMustUseForkId);
        }
    }

    Ok(())
}

fn check_pubkey_encoding(v: &[u8], flags: &VerificationFlags) -> Result<(), Error> {
    if flags.verify_strictenc && !is_public_key(v) {
        return Err(Error::PubkeyType);
    }

    Ok(())
}

fn check_minimal_push(data: &[u8], opcode: Opcode) -> bool {
    if data.is_empty() {
        // Could have used OP_0.
        opcode == Opcode::OP_0
    } else if data.len() == 1 && data[0] >= 1 && data[0] <= 16 {
        // Could have used OP_1 .. OP_16.
        opcode as u8 == Opcode::OP_1 as u8 + (data[0] - 1)
    } else if data.len() == 1 && data[0] == 0x81 {
        // Could have used OP_1NEGATE
        opcode == Opcode::OP_1NEGATE
    } else if data.len() <= 75 {
        // Could have used a direct push (opcode indicating number of bytes pushed + those bytes).
        opcode as usize == data.len()
    } else if data.len() <= 255 {
        // Could have used OP_PUSHDATA.
        opcode == Opcode::OP_PUSHDATA1
    } else if data.len() <= 65535 {
        // Could have used OP_PUSHDATA2.
        opcode == Opcode::OP_PUSHDATA2
    } else {
        true
    }
}

fn cast_to_bool(data: &[u8]) -> bool {
    if data.is_empty() {
        return false;
    }

    if data[..data.len() - 1].iter().any(|x| x != &0) {
        return true;
    }

    let last = data[data.len() - 1];
    !(last == 0 || last == 0x80)
}
