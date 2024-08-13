use substrate_prometheus_endpoint::prometheus::IntCounterVec;
use substrate_prometheus_endpoint::{register, GaugeVec, Opts, PrometheusError, Registry, U64};

#[derive(Clone)]
pub struct Metrics {
    pub(crate) addresses: GaugeVec<U64>,
    pub(crate) bandwidth: GaugeVec<U64>,
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
            bandwidth: register(
                GaugeVec::new(
                    Opts::new("subcoin_p2p_bandwidth", "Subcoin network bandwidth"),
                    &["direction"],
                )?,
                registry,
            )?,

            connected_peers: register(
                GaugeVec::new(
                    Opts::new("subcoin_p2p_connected_peers", "Number of connected peers"),
                    &["direction"],
                )?,
                registry,
            )?,
            messages_received: register(
                IntCounterVec::new(
                    Opts::new(
                        "subcoin_p2p_messages_received_total",
                        "Subcoin network messages received",
                    ),
                    &["type"],
                )?,
                registry,
            )?,
            messages_sent: register(
                IntCounterVec::new(
                    Opts::new(
                        "subcoin_p2p_messages_sent_total",
                        "Subcoin network messages sent",
                    ),
                    &["type"],
                )?,
                registry,
            )?,
        })
    }
}
