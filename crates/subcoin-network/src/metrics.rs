use substrate_prometheus_endpoint::prometheus::IntCounterVec;
use substrate_prometheus_endpoint::{GaugeVec, Opts, PrometheusError, Registry, U64, register};

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
        })
    }
}
