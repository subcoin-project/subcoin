pub use bitcoin::constants::MAX_SCRIPT_ELEMENT_SIZE;
use num_traits::Num;
use std::sync::LazyLock;

// pub constant for compressed public key size
pub const COMPRESSED_PUBKEY_SIZE: usize = 33;

pub const WITNESS_V0_SCRIPTHASH_SIZE: usize = 32;
pub const WITNESS_V0_KEYHASH_SIZE: usize = 20;
pub const WITNESS_V0_TAPROOT_SIZE: usize = 32;

/// The maximum combined height of stack and alt stack during script execution.
pub const MAX_STACK_SIZE: usize = 1000;

pub const SCRIPT_VERIFY_DISCOURAGE_OP_SUCCESS: u32 = 1 << 0;

/// Maximum number of public keys per multisig.
pub const MAX_PUBKEYS_PER_MULTISIG: i64 = 20;

/// Maximum number of non-push operations per script.
pub const MAX_OPS_PER_SCRIPT: usize = 201;

pub const SIGHASH_ALL: u8 = 0x01;
pub const SIGHASH_NONE: u8 = 0x02;
pub const SIGHASH_SINGLE: u8 = 0x03;
pub const SIGHASH_ANYONECANPAY: u8 = 0x80;

/// Below flags apply in the context of BIP 68
/// If this flag set, CTxIn::nSequence is NOT interpreted as a relative lock-time.
pub const SEQUENCE_LOCKTIME_DISABLE_FLAG: u32 = 1u32 << 31;

/// Validation weight per passsing signature (Tapscript only, see BIP 342).
pub const VALIDATION_WEIGHT_PER_SIGOP_PASSED: i64 = 50;

pub static HALF_ORDER: LazyLock<num_bigint::BigInt> = LazyLock::new(|| {
    pub const N: &str = "fffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141";
    num_bigint::BigInt::from_str_radix(N, 16).expect("Static value must be valid")
});
