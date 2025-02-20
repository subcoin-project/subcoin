use std::time::Duration;

/// Default setting for -datacarriersize.
/// 80 bytes of data, +1 for OP_RETURN, +2 for the pushdata opcodes.
pub const MAX_OP_RETURN_RELAY: usize = 83;

/// Configuration options for the transaction memory pool.
#[derive(Clone, Debug)]
pub struct MemPoolLimits {
    /// Maximum number of ancestors for a transaction
    pub max_ancestors: usize,

    /// Maximum size (in virtual bytes) of all ancestors for a transaction
    pub max_ancestor_size: usize,

    /// Maximum number of descendants for a transaction
    pub max_descendants: usize,

    /// Maximum size (in virtual bytes) of all descendants for a transaction
    pub max_descendant_size: usize,
}

impl Default for MemPoolLimits {
    fn default() -> Self {
        Self {
            max_ancestors: 25,
            max_ancestor_size: 101_000,
            max_descendants: 25,
            max_descendant_size: 101_000,
        }
    }
}

/// Configuration options for the transaction memory pool.
#[derive(Clone, Debug)]
pub struct MemPoolOptions {
    /// Maximum size of the mempool in MB (default: 300)
    pub max_size_mb: usize,

    /// Number of hours to keep transactions in the mempool
    pub expiry_hours: u32,

    /// Minimum fee rate in satoshis per KB for a transaction to be accepted
    pub min_relay_fee_rate: u64,

    /// Minimum fee rate in satoshis per KB for a transaction to be accepted
    pub dust_relay_fee: u64,

    /// Whether to require standard transactions
    pub require_standard: bool,

    /// Whether to require standard transactions
    pub permit_bare_multisig: bool,

    /// Maximum script verification cache size
    pub max_script_cache_size: usize,

    pub limits: MemPoolLimits,
}

impl Default for MemPoolOptions {
    fn default() -> Self {
        Self {
            max_size_mb: 300,
            expiry_hours: 336,        // 2 weeks
            min_relay_fee_rate: 1000, // 1 sat/byte
            dust_relay_fee: 1000,     // 1 sat/byte
            permit_bare_multisig: bool,
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
        self.max_size_mb * 1_000_000
    }

    /// Get the expiry duration
    pub fn expiry_duration(&self) -> Duration {
        Duration::from_secs(self.expiry_hours as u64 * 3600)
    }
}

/// Builder pattern for MemPoolOptions
#[derive(Default)]
pub struct MemPoolOptionsBuilder {
    options: MemPoolOptions,
}

impl MemPoolOptionsBuilder {
    /// Set maximum size of the mempool in MB
    pub fn max_size_mb(mut self, size: usize) -> Self {
        self.options.max_size_mb = size;
        self
    }

    /// Set minimum relay fee rate
    pub fn min_relay_fee_rate(mut self, rate: u64) -> Self {
        self.options.min_relay_fee_rate = rate;
        self
    }

    /// Set expiry time in hours
    pub fn expiry_hours(mut self, hours: u32) -> Self {
        self.options.expiry_hours = hours;
        self
    }

    /// Set maximum number of ancestors
    pub fn max_ancestors(mut self, count: usize) -> Self {
        self.options.limits.max_ancestors = count;
        self
    }

    /// Set maximum ancestor size
    pub fn max_ancestor_size(mut self, size: usize) -> Self {
        self.options.limits.max_ancestor_size = size;
        self
    }

    /// Set maximum number of descendants
    pub fn max_descendants(mut self, count: usize) -> Self {
        self.options.limits.max_descendants = count;
        self
    }

    /// Set maximum descendant size
    pub fn max_descendant_size(mut self, size: usize) -> Self {
        self.options.limits.max_descendant_size = size;
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
            .max_size_mb(500)
            .min_relay_fee_rate(2000)
            .expiry_hours(168) // 1 week
            .max_ancestors(50)
            .build();

        assert_eq!(options.max_size_mb, 500);
        assert_eq!(options.min_relay_fee_rate, 2000);
        assert_eq!(options.expiry_hours, 168);
        assert_eq!(options.max_ancestors, 50);
    }

    #[test]
    fn test_mempool_options_defaults() {
        let options = MemPoolOptions::default();

        assert_eq!(options.max_size_mb, 300);
        assert_eq!(options.min_relay_fee_rate, 1000);
        assert_eq!(options.expiry_hours, 336);
        assert_eq!(options.max_ancestors, 25);
    }
}
