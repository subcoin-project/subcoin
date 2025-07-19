use substrate_prometheus_endpoint::{GaugeVec, Opts, PrometheusError, Registry, U64, register};

pub struct Metrics {
    block_execution_time: GaugeVec<U64>,
    block_transactions_count: GaugeVec<U64>,
    block_size: GaugeVec<U64>,
}

impl Metrics {
    pub fn register(registry: &Registry) -> Result<Self, PrometheusError> {
        Ok(Self {
            block_execution_time: register(
                GaugeVec::new(
                    Opts::new(
                        "subcoin_block_execution_time_milliseconds",
                        "Time taken to execute a block in milliseconds",
                    ),
                    &["block_height"],
                )?,
                registry,
            )?,
            block_transactions_count: register(
                GaugeVec::new(
                    Opts::new(
                        "subcoin_block_transactions_count",
                        "Number of transactions in the block",
                    ),
                    &["block_height"],
                )?,
                registry,
            )?,
            block_size: register(
                GaugeVec::new(
                    Opts::new("subcoin_block_size", "Block size in bytes"),
                    &["block_height"],
                )?,
                registry,
            )?,
        })
    }

    pub fn report_block_execution(
        &self,
        block_height: u32,
        transactions_count: usize,
        block_size: usize,
        execution_time: u128,
    ) {
        let block_height = block_height.to_string();
        self.block_transactions_count
            .with_label_values(&[&block_height])
            .set(transactions_count as u64);
        self.block_size
            .with_label_values(&[&block_height])
            .set(block_size as u64);
        self.block_execution_time
            .with_label_values(&[&block_height])
            .set(execution_time as u64);
    }
}
