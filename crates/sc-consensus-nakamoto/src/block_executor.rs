/// A simply way to track the overall execution info for optimization purpose.
#[derive(Debug, Default)]
pub struct ExecutionInfo {
    /// Time taken by `runtime_api.execute_block` in nanoseconds.
    pub execute_block: u128,
    /// Time taken by `client.state_at` in nanoseconds.
    pub fetch_state: u128,
    /// Time taken by `runtime_api.into_storage_changes` in nanoseconds.
    pub into_storage_changes: u128,
}

impl ExecutionInfo {
    /// Returns the total execution time in nanoseconds.
    pub fn total(&self) -> u128 {
        self.execute_block + self.fetch_state + self.into_storage_changes
    }
}
