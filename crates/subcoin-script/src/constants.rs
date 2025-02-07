pub use bitcoin::constants::MAX_SCRIPT_ELEMENT_SIZE;
use num_bigint::BigInt;
use num_traits::Num;
use std::sync::LazyLock;

/// Size of the compressed public key.
pub const COMPRESSED_PUBKEY_SIZE: usize = 33;

/// Size of script hash.
pub const WITNESS_V0_SCRIPTHASH_SIZE: usize = 32;
/// Size of key hash.
pub const WITNESS_V0_KEYHASH_SIZE: usize = 20;
/// Size of taproot.
pub const WITNESS_V0_TAPROOT_SIZE: usize = 32;

/// The maximum combined height of stack and alt stack during script execution.
pub const MAX_STACK_SIZE: usize = 1000;

/// Maximum number of public keys per multisig.
pub const MAX_PUBKEYS_PER_MULTISIG: i64 = 20;

/// Maximum number of non-push operations per script.
pub const MAX_OPS_PER_SCRIPT: usize = 201;

/// Below flags apply in the context of BIP 68
/// If this flag set, CTxIn::nSequence is NOT interpreted as a relative lock-time.
pub const SEQUENCE_LOCKTIME_DISABLE_FLAG: u32 = 1u32 << 31;

/// Validation weight per passsing signature (Tapscript only, see BIP 342).
pub const VALIDATION_WEIGHT_PER_SIGOP_PASSED: i64 = 50;

/// Validation weight per passing signature (Tapscript only, see BIP 342)
pub const VALIDATION_WEIGHT_OFFSET: i64 = 50;

/// Half order of secp256k1.
pub static HALF_ORDER: LazyLock<BigInt> = LazyLock::new(|| {
    pub const N: &str = "FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141";
    let full_order = BigInt::from_str_radix(N, 16).expect("Full order of secp256k1 must be valid");
    full_order >> 1
});
