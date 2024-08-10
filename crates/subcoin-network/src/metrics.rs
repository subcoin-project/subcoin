use substrate_prometheus_endpoint::{register, GaugeVec, Opts, PrometheusError, Registry, U64};

#[derive(Clone)]
pub struct Metrics {
    pub(crate) connected_peers: GaugeVec<U64>,
    pub(crate) addresses: GaugeVec<U64>,
    pub(crate) bandwidth: GaugeVec<U64>,
}

impl Metrics {
    pub fn register(registry: &Registry) -> Result<Self, PrometheusError> {
        Ok(Self {
            connected_peers: register(
                GaugeVec::new(
                    Opts::new("subcoin_p2p_connected_peers", "Number of connected peers"),
                    &["status"],
                )?,
                registry,
            )?,
            addresses: register(
                GaugeVec::new(Opts::new("subcoin_p2p_addresses", "Addresses"), &["status"])?,
                registry,
            )?,
            bandwidth: register(
                GaugeVec::new(
                    Opts::new("subcoin_p2p_bandwidth", "Network bandwidth"),
                    &["direction"],
                )?,
                registry,
            )?,
        })
    }
}
