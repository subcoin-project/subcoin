use substrate_prometheus_endpoint::{register, Gauge, PrometheusError, Registry, U64};

pub struct Metrics {
    connected_peers: Gauge<U64>,
}

impl Metrics {
    pub fn register(registry: &Registry) -> Result<Self, PrometheusError> {
        Ok(Self {
            connected_peers: register(
                Gauge::new(
                    "subcoin_p2p_connected_peers",
                    "Number of connected peers in subcoin networking",
                )?,
                registry,
            )?,
        })
    }

    pub fn report_connected_peers(&self, count: usize) {
        self.connected_peers.set(count as u64);
    }
}
