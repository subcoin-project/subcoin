//! # Bitcoin Script Interpreter
//!
//! This crate implements a Bitcoin Script interpreter in Rust. It provides functionality to
//! interpret and evaluate Bitcoin scripts, similar to the Bitcoin Core implementation, but with a
//! focus on readability and compatibility with Rust. Performance optimizations will be pursued
//! in future updates.
//!
//! ## Key Points
//!
//! - Some functions are directly ported from Bitcoin Core and may not follow typical Rust idioms.
//!   They are intentionally written in a way that is closer to the original C++ code to preserve
//!   functionality and logic.
//!
//! - Several components, including tests prior to the Taproot upgrade, are ported from the
//!   Parity-Bitcoin project to reuse their valuable work for Bitcoin's older features and standards.
//!
//! ## Features
//!
//! This crate aims to provide:
//!
//! - Support for interpreting Bitcoin's standard script types (P2PKH, P2SH, P2WPKH, etc.).
//! - Efficient validation of Bitcoin script execution, including signature verification.
//! - Compatibility with Bitcoin Core's opcodes and evaluation logic, with Rust-specific optimizations.
//!
//! ## Caveats
//!
//! This library is **not widely used** and **lacks comprehensive tests**. As a result, it is **never use it for production use**!
//! Please use it with caution, and only in non-critical applications or for experimentation purposes.

mod constants;
mod error;
mod interpreter;
mod num;
mod opcode;
mod signature_checker;
mod stack;
#[cfg(test)]
mod tests;

use bitcoin::hashes::Hash;
use bitcoin::{secp256k1, TapLeafHash};
use bitflags::bitflags;

pub use self::error::Error;
pub use self::interpreter::verify_script;
pub use self::signature_checker::{
    NoSignatureCheck, SignatureChecker, SignatureError, TransactionSignatureChecker,
};

pub type H256 = bitcoin::hashes::sha256::Hash;
pub type SchnorrSignature = bitcoin::taproot::Signature;

/// Same semantic with [`bitcoin::ecdsa::Signature`] with the following differences:
///
/// - `sighash_type` uses u32 instead of [`bitcoin::EcdsaSighashType`].
/// - Ensure lower S via `normalize_s()`.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct EcdsaSignature {
    /// The underlying ECDSA Signature.
    pub signature: secp256k1::ecdsa::Signature,
    /// The corresponding hash type.
    pub sighash_type: u32,
}

impl EcdsaSignature {
    /// Constructs a [`EcdsaSignature`] from the full sig bytes.
    ///
    /// https://github.com/bitcoin/bitcoin/blob/82ba50513425bf0568d4f9456282dc9713132490/src/pubkey.cpp#L285
    /// https://github.com/bitcoin/bitcoin/blob/82ba50513425bf0568d4f9456282dc9713132490/src/pubkey.cpp#L290
    pub fn parse_der_lax(full_sig_bytes: &[u8]) -> Result<Self, bitcoin::ecdsa::Error> {
        let (sighash_type, sig) = full_sig_bytes
            .split_last()
            .ok_or(bitcoin::ecdsa::Error::EmptySignature)?;
        let sighash_type = *sighash_type as u32;

        let mut signature = secp256k1::ecdsa::Signature::from_der_lax(sig)?;

        // libsecp256k1's ECDSA verification requires lower-S signatures, which have
        // not historically been enforced in Bitcoin, so normalize them first.
        signature.normalize_s();

        Ok(Self {
            signature,
            sighash_type,
        })
    }
}

bitflags! {
    /// Script verification flags.
    ///
    /// https://github.com/bitcoin/bitcoin/blob/6f9db1ebcab4064065ccd787161bf2b87e03cc1f/src/script/interpreter.h#L45
    #[derive(Debug, Clone)]
    pub struct VerifyFlags: u32 {
        const NONE = 0;

        /// Evaluate P2SH subscripts (BIP16).
        const P2SH = 1 << 0;

        /// Passing a non-strict-DER signature or one with undefined hashtype to a checksig operation causes script failure.
        /// Evaluating a pubkey that is not (0x04 + 64 bytes) or (0x02 or 0x03 + 32 bytes) by checksig causes script failure.
        /// (not used or intended as a consensus rule).
        const STRICTENC = 1 << 1;

        // Passing a non-strict-DER signature to a checksig operation causes script failure (BIP62 rule 1)
        const DERSIG = 1 << 2;

        // Passing a non-strict-DER signature or one with S > order/2 to a checksig operation causes script failure
        // (BIP62 rule 5)
        const LOW_S = 1 << 3;

        // verify dummy stack item consumed by CHECKMULTISIG is of zero-length (BIP62 rule 7).
        const NULLDUMMY = 1 << 4;

        // Using a non-push operator in the scriptSig causes script failure (BIP62 rule 2).
        const SIGPUSHONLY = 1 << 5;

        // Require minimal encodings for all push operations (OP_0... OP_16, OP_1NEGATE where possible, direct
        // pushes up to 75 bytes, OP_PUSHDATA up to 255 bytes, OP_PUSHDATA2 for anything larger). Evaluating
        // any other push causes the script to fail (BIP62 rule 3).
        // In addition, whenever a stack element is interpreted as a number, it must be of minimal length (BIP62 rule 4).
        const MINIMALDATA = 1 << 6;

        // Discourage use of NOPs reserved for upgrades (NOP1-10)
        //
        // Provided so that nodes can avoid accepting or mining transactions
        // containing executed NOP's whose meaning may change after a soft-fork,
        // thus rendering the script invalid; with this flag set executing
        // discouraged NOPs fails the script. This verification flag will never be
        // a mandatory flag applied to scripts in a block. NOPs that are not
        // executed, e.g.  within an unexecuted IF ENDIF block, are *not* rejected.
        // NOPs that have associated forks to give them new meaning (CLTV, CSV)
        // are not subject to this rule.
        const DISCOURAGE_UPGRADABLE_NOPS = 1 << 7;

        // Require that only a single stack element remains after evaluation. This changes the success criterion from
        // "At least one stack element must remain, and when interpreted as a boolean, it must be true" to
        // "Exactly one stack element must remain, and when interpreted as a boolean, it must be true".
        // (BIP62 rule 6)
        // Note: CLEANSTACK should never be used without P2SH or WITNESS.
        // Note: WITNESS_V0 and TAPSCRIPT script execution have behavior similar to CLEANSTACK as part of their
        //       consensus rules. It is automatic there and does not need this flag.
        const CLEANSTACK = 1 << 8;

        // Verify CHECKLOCKTIMEVERIFY
        //
        // See BIP65 for details.
        const CHECKLOCKTIMEVERIFY = 1 << 9;

        // support CHECKSEQUENCEVERIFY opcode
        //
        // See BIP112 for details
        const CHECKSEQUENCEVERIFY = 1 << 10;

        // Support segregated witness
        const WITNESS = 1 << 11;

        // Making v1-v16 witness program non-standard
        const DISCOURAGE_UPGRADABLE_WITNESS_PROGRAM = 1 << 12;

        // Segwit script only: Require the argument of OP_IF/NOTIF to be exactly 0x01 or empty vector
        //
        // Note: TAPSCRIPT script execution has behavior similar to MINIMALIF as part of its consensus
        //       rules. It is automatic there and does not depend on this flag.
        const MINIMALIF = 1 << 13;

        // Signature(s) must be empty vector if a CHECK(MULTI)SIG operation failed
        const NULLFAIL = 1 << 14;

        // Public keys in segregated witness scripts must be compressed
        const WITNESS_PUBKEYTYPE = 1 << 15;

        // Making OP_CODESEPARATOR and FindAndDelete fail any non-segwit scripts
        const CONST_SCRIPTCODE = 1 << 16;

        // Taproot/Tapscript validation (BIPs 341 & 342)
        const TAPROOT = 1 << 17;

        // Making unknown Taproot leaf versions non-standard
        const DISCOURAGE_UPGRADABLE_TAPROOT_VERSION = 1 << 18;

        // Making unknown OP_SUCCESS non-standard
        const DISCOURAGE_OP_SUCCESS = 1 << 19;

        // Making unknown public key versions (in BIP 342 scripts) non-standard
        const DISCOURAGE_UPGRADABLE_PUBKEYTYPE = 1 << 20;
    }
}

impl VerifyFlags {
    pub fn verify_minimaldata(&self) -> bool {
        self.contains(Self::MINIMALDATA)
    }

    pub fn verify_sigpushonly(&self) -> bool {
        self.contains(Self::SIGPUSHONLY)
    }

    pub fn verify_p2sh(&self) -> bool {
        self.contains(Self::P2SH)
    }

    pub fn verify_witness(&self) -> bool {
        self.contains(Self::WITNESS)
    }

    pub fn verify_discourage_upgradable_witness_program(&self) -> bool {
        self.contains(Self::DISCOURAGE_UPGRADABLE_WITNESS_PROGRAM)
    }
}

/// Represents different signature verification schemes used in Bitcoin
///
/// https://github.com/bitcoin/bitcoin/blob/6f9db1ebcab4064065ccd787161bf2b87e03cc1f/src/script/interpreter.h#L190
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigVersion {
    /// Bare scripts and BIP16 P2SH-wrapped redeemscripts
    Base,
    /// Witness v0 (P2WPKH and P2WSH); see BIP 141
    WitnessV0,
    /// Witness v1 with 32-byte program, not BIP16 P2SH-wrapped, key path spending; see BIP 341
    Taproot,
    /// Witness v1 with 32-byte program, not BIP16 P2SH-wrapped, script path spending,
    /// leaf version 0xc0; see BIP 342
    Tapscript,
}

// https://github.com/bitcoin/bitcoin/blob/6f9db1ebcab4064065ccd787161bf2b87e03cc1f/src/script/interpreter.h#L198
#[derive(Debug)]
pub struct ScriptExecutionData {
    /// Whether m_tapleaf_hash is initialized
    pub tapleaf_hash_init: bool,
    /// The tapleaf hash
    pub tapleaf_hash: TapLeafHash,

    /// Whether m_codeseparator_pos is initialized
    pub codeseparator_pos_init: bool,
    /// Opcode position of the last executed OP_CODESEPARATOR (or 0xFFFFFFFF if none executed)
    pub codeseparator_pos: u32,

    /// Whether m_annex_present and m_annex_hash are initialized
    pub annex_init: bool,
    /// Whether an annex is present
    pub annex_present: bool,
    /// Hash of the annex data
    pub annex_hash: H256,
    /// Annex data.
    ///
    /// We store the annex data for signature_checker.
    pub annex: Option<Vec<u8>>,

    /// Whether m_validation_weight_left is initialized
    pub validation_weight_left_init: bool,
    /// How much validation weight is left (decremented for every successful non-empty signature check)
    pub validation_weight_left: i64,

    /// The hash of the corresponding output
    pub output_hash: Option<H256>,
}

impl Default for ScriptExecutionData {
    fn default() -> Self {
        Self {
            tapleaf_hash_init: false,
            tapleaf_hash: TapLeafHash::from_slice(H256::all_zeros().as_byte_array())
                .expect("Static value must be correct; qed"),
            codeseparator_pos_init: false,
            codeseparator_pos: 0,
            annex_init: false,
            annex_present: false,
            annex_hash: H256::all_zeros(),
            annex: None,
            validation_weight_left_init: false,
            validation_weight_left: 0,
            output_hash: None,
        }
    }
}
