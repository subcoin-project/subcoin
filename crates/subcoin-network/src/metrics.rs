use substrate_prometheus_endpoint::prometheus::{IntCounter, IntCounterVec};
use substrate_prometheus_endpoint::{
    Gauge, GaugeVec, Opts, PrometheusError, Registry, U64, register,
};

#[derive(Clone, Debug)]
pub struct BandwidthMetrics {
    pub(crate) bandwidth: GaugeVec<U64>,
}

impl BandwidthMetrics {
    pub fn register(registry: &Registry) -> Result<Self, PrometheusError> {
        Ok(Self {
            bandwidth: register(
                GaugeVec::new(
                    Opts::new(
                        "subcoin_p2p_bandwidth_bytes",
                        "Network bandwidth usage in bytes",
                    ),
                    &["direction"],
                )?,
                registry,
            )?,
        })
    }
}

#[derive(Clone)]
pub struct Metrics {
    pub(crate) addresses: GaugeVec<U64>,
    pub(crate) connected_peers: GaugeVec<U64>,
    pub(crate) messages_received: IntCounterVec,
    pub(crate) messages_sent: IntCounterVec,

    // Queue status metrics
    pub(crate) queue_saturation_ratio: Gauge<U64>,
    pub(crate) queue_block_count: Gauge<U64>,
    pub(crate) queue_memory_bytes: Gauge<U64>,

    // Sync progress metrics
    pub(crate) sync_target_height: Gauge<U64>,
    pub(crate) blocks_imported_total: IntCounter,

    // Header prefetch metrics
    pub(crate) header_prefetch_hits: IntCounter,
    pub(crate) header_prefetch_misses: IntCounter,
}

impl Metrics {
    pub fn register(registry: &Registry) -> Result<Self, PrometheusError> {
        Ok(Self {
            addresses: register(
                GaugeVec::new(
                    Opts::new("subcoin_p2p_addresses", "Bitcoin node addresses"),
                    &["status"],
                )?,
                registry,
            )?,
            connected_peers: register(
                GaugeVec::new(
                    Opts::new(
                        "subcoin_p2p_connected_peers_total",
                        "Total number of connected peers",
                    ),
                    &["direction"],
                )?,
                registry,
            )?,
            messages_received: register(
                IntCounterVec::new(
                    Opts::new(
                        "subcoin_p2p_messages_received_total",
                        "Total number of network messages received",
                    ),
                    &["type"],
                )?,
                registry,
            )?,
            messages_sent: register(
                IntCounterVec::new(
                    Opts::new(
                        "subcoin_p2p_messages_sent_total",
                        "Total number of network messages sent",
                    ),
                    &["type"],
                )?,
                registry,
            )?,

            // Queue status metrics
            queue_saturation_ratio: register(
                Gauge::new(
                    "subcoin_queue_saturation_ratio",
                    "Percentage of time import queue is saturated (0-100)",
                )?,
                registry,
            )?,
            queue_block_count: register(
                Gauge::new(
                    "subcoin_queue_block_count",
                    "Number of blocks in import queue",
                )?,
                registry,
            )?,
            queue_memory_bytes: register(
                Gauge::new(
                    "subcoin_queue_memory_bytes",
                    "Memory used by queued blocks in bytes",
                )?,
                registry,
            )?,

            // Sync progress metrics
            sync_target_height: register(
                Gauge::new(
                    "subcoin_sync_target_height",
                    "Target block height from sync peer",
                )?,
                registry,
            )?,
            blocks_imported_total: register(
                IntCounter::new(
                    "subcoin_blocks_imported_total",
                    "Total blocks imported since start",
                )?,
                registry,
            )?,

            // Header prefetch metrics
            header_prefetch_hits: register(
                IntCounter::new(
                    "subcoin_header_prefetch_hits_total",
                    "Number of times block download started immediately because headers were prefetched",
                )?,
                registry,
            )?,
            header_prefetch_misses: register(
                IntCounter::new(
                    "subcoin_header_prefetch_misses_total",
                    "Number of times block download had to wait for headers",
                )?,
                registry,
            )?,
        })
    }
}
