use substrate_prometheus_endpoint::{register, GaugeVec, Opts, PrometheusError, Registry, U64};

pub struct Metrics {
    block_execution_time: GaugeVec<U64>,
}

impl Metrics {
    pub fn register(registry: &Registry) -> Result<Self, PrometheusError> {
        Ok(Self {
            block_execution_time: register(
                GaugeVec::new(
                    Opts::new(
                        "block_execution_time_milliseconds",
                        "Time taken to execute a block in milliseconds",
                    ),
                    &["block_height"],
                )?,
                registry,
            )?,
        })
    }

    pub fn report_block_execution_time(&self, block_height: u32, execution_time: u128) {
        self.block_execution_time
            .with_label_values(&[&block_height.to_string()])
            .set(execution_time as u64);
    }
}
