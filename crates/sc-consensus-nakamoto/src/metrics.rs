use substrate_prometheus_endpoint::{
    register, Histogram, HistogramOpts, PrometheusError, Registry,
};

pub struct Metrics {
    block_execution_time: Histogram,
}

impl Metrics {
    pub fn register(registry: &Registry) -> Result<Self, PrometheusError> {
        Ok(Self {
            block_execution_time: register(
                Histogram::with_opts(HistogramOpts::new(
                    "block_execution_time_milliseconds",
                    "Time taken to execute a block in milliseconds",
                ))?,
                registry,
            )?,
        })
    }

    pub fn report_block_execution_time(&self, execution_time: u128) {
        self.block_execution_time.observe(execution_time as f64);
    }
}
