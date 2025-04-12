use std::time::Duration;

/** Default for -maxmempool, maximum megabytes of mempool memory usage */
const DEFAULT_MAX_MEMPOOL_SIZE_MB: usize = 300;

/** Default for -mempoolexpiry, expiration time for mempool transactions in hours */
const DEFAULT_MEMPOOL_EXPIRY_HOURS: u32 = 336;

/** Default for -acceptnonstdtxn */
const DEFAULT_ACCEPT_NON_STD_TXN: bool = false;

/// Default setting for -datacarriersize.
/// 80 bytes of data, +1 for OP_RETURN, +2 for the pushdata opcodes.
pub const MAX_OP_RETURN_RELAY: usize = 83;

/// Configuration options for the transaction memory pool.
#[derive(Clone, Debug)]
pub struct MemPoolLimits {
    /// Maximum number of ancestors for a transaction
    pub max_ancestors: usize,

    /// Maximum size (in virtual bytes) of all ancestors for a transaction
    pub max_ancestor_size_vbytes: usize,

    /// Maximum number of descendants for a transaction
    pub max_descendants: usize,

    /// Maximum size (in virtual bytes) of all descendants for a transaction
    pub max_descendant_size_vbytes: usize,
}

impl Default for MemPoolLimits {
    fn default() -> Self {
        Self {
            max_ancestors: 25,
            max_ancestor_size_vbytes: 101_000,
            max_descendants: 25,
            max_descendant_size_vbytes: 101_000,
        }
    }
}

/// Configuration options for the transaction memory pool.
#[derive(Clone, Debug)]
pub struct MemPoolOptions {
    /// Maximum size of the mempool in bytes (default: 300 * 1_000_000)
    pub max_size_bytes: usize,

    /// Number of seconds to keep transactions in the mempool (default: 336 * 60 * 60)
    pub expiry_seconds: u32,

    /// Minimum fee rate in satoshis per KB for a transaction to be accepted.
    pub min_relay_feerate: u64,

    /// Minimum fee rate in satoshis per KB for a transaction to be accepted.
    pub dust_relay_feerate: u64,

    /// > A data carrying output is an unspendable output containing data. The script
    /// > type is designated as TxoutType::NULL_DATA.
    /// >
    /// > Maximum size of TxoutType::NULL_DATA scripts that this node considers standard.
    /// > If nullopt, any size is nonstandard.
    pub max_datacarrier_bytes: usize,

    /// Whether to require standard transactions
    pub permit_bare_multisig: bool,

    /// Whether to require standard transactions
    pub require_standard: bool,

    /// Maximum script verification cache size
    pub max_script_cache_size: usize,

    pub limits: MemPoolLimits,
}

impl Default for MemPoolOptions {
    fn default() -> Self {
        Self {
            max_size_bytes: DEFAULT_MAX_MEMPOOL_SIZE_MB * 1_000_000,
            expiry_seconds: DEFAULT_MEMPOOL_EXPIRY_HOURS * 60 * 60, // 2 weeks
            min_relay_feerate: 1000,                                // 1 sat/byte
            dust_relay_feerate: 1000,                               // 1 sat/byte
            max_datacarrier_bytes: MAX_OP_RETURN_RELAY,
            permit_bare_multisig: true,
            require_standard: true,
            max_script_cache_size: 32 << 20, // 32 MB
            limits: MemPoolLimits::default(),
        }
    }
}

impl MemPoolOptions {
    /// Create new mempool options with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a builder for configuring mempool options
    pub fn builder() -> MemPoolOptionsBuilder {
        MemPoolOptionsBuilder::default()
    }

    /// Get the maximum size of the mempool in bytes
    pub fn max_size_bytes(&self) -> usize {
        self.max_size_bytes * 1_000_000
    }

    /// Get the expiry duration
    pub fn expiry_duration(&self) -> Duration {
        Duration::from_secs(self.expiry_seconds as u64 * 3600)
    }
}

/// Builder pattern for MemPoolOptions
#[derive(Default)]
pub struct MemPoolOptionsBuilder {
    options: MemPoolOptions,
}

impl MemPoolOptionsBuilder {
    /// Set maximum size of the mempool in MB
    pub fn max_size_bytes(mut self, size: usize) -> Self {
        self.options.max_size_bytes = size;
        self
    }

    /// Set minimum relay fee rate
    pub fn min_relay_feerate(mut self, rate: u64) -> Self {
        self.options.min_relay_feerate = rate;
        self
    }

    /// Set expiry time in hours
    pub fn expiry_seconds(mut self, hours: u32) -> Self {
        self.options.expiry_seconds = hours;
        self
    }

    /// Set maximum number of ancestors
    pub fn max_ancestors(mut self, count: usize) -> Self {
        self.options.limits.max_ancestors = count;
        self
    }

    /// Set maximum ancestor size
    pub fn max_ancestor_size_vbytes(mut self, size: usize) -> Self {
        self.options.limits.max_ancestor_size_vbytes = size;
        self
    }

    /// Set maximum number of descendants
    pub fn max_descendants(mut self, count: usize) -> Self {
        self.options.limits.max_descendants = count;
        self
    }

    /// Set maximum descendant size
    pub fn max_descendant_size_vbytes(mut self, size: usize) -> Self {
        self.options.limits.max_descendant_size_vbytes = size;
        self
    }

    /// Set whether to require standard transactions
    pub fn require_standard(mut self, require: bool) -> Self {
        self.options.require_standard = require;
        self
    }

    /// Build the final MemPoolOptions
    pub fn build(self) -> MemPoolOptions {
        self.options
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mempool_options_builder() {
        let options = MemPoolOptions::builder()
            .max_size_bytes(500)
            .min_relay_feerate(2000)
            .expiry_seconds(168) // 1 week
            .max_ancestors(50)
            .build();

        assert_eq!(options.max_size_bytes, 500);
        assert_eq!(options.min_relay_feerate, 2000);
        assert_eq!(options.expiry_seconds, 168);
        assert_eq!(options.limits.max_ancestors, 50);
    }

    #[test]
    fn test_mempool_options_defaults() {
        let options = MemPoolOptions::default();

        assert_eq!(options.max_size_bytes, 300);
        assert_eq!(options.min_relay_feerate, 1000);
        assert_eq!(options.expiry_seconds, 336);
        assert_eq!(options.limits.max_ancestors, 25);
    }
}
