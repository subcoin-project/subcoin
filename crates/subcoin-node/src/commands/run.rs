use crate::cli::params::{CommonParams, NetworkParams};
use clap::Parser;
use sc_cli::{DatabasePruningMode, NodeKeyParams, PruningParams, Role, SharedParams};
use sc_consensus_nakamoto::BlockVerification;
use sc_service::BlocksPruning;
use subcoin_network::SyncStrategy;

/// The `run` command used to run a Bitcoin node.
#[derive(Debug, Clone, Parser)]
pub struct Run {
    /// Specify the major sync strategy.
    #[clap(long, value_parser = clap::value_parser!(SyncStrategy))]
    pub sync_strategy: SyncStrategy,

    /// Specify the block verification level.
    #[clap(long, value_parser = clap::value_parser!(BlockVerification), default_value = "full")]
    pub block_verification: BlockVerification,

    /// Do not run the finalizer which will finalize the blocks on confirmation depth.
    #[clap(long)]
    pub no_finalizer: bool,

    #[allow(missing_docs)]
    #[clap(flatten)]
    pub common_params: CommonParams,

    #[allow(missing_docs)]
    #[clap(flatten)]
    pub network_params: NetworkParams,
}

impl Run {
    pub fn subcoin_network_params(&self, network: bitcoin::Network) -> subcoin_network::Params {
        subcoin_network::Params {
            network,
            listen_on: self.network_params.listen.clone(),
            bootnodes: self.network_params.bootnode.clone(),
            bootnode_only: self.network_params.bootnode_only,
            ipv4_only: self.network_params.ipv4_only,
            max_outbound_peers: self.network_params.max_outbound_peers,
            max_inbound_peers: self.network_params.max_inbound_peers,
            sync_strategy: self.sync_strategy,
        }
    }
}

/// Adapter of [`sc_cli::RunCmd`].
pub struct RunCmd {
    shared_params: SharedParams,
    pruning_params: PruningParams,
}

impl RunCmd {
    pub fn new(run: &Run) -> Self {
        let shared_params = run.common_params.as_shared_params();
        let pruning_params = PruningParams {
            state_pruning: Some(DatabasePruningMode::Archive),
            blocks_pruning: DatabasePruningMode::Archive,
        };
        Self {
            shared_params,
            pruning_params,
        }
    }
}

impl sc_cli::CliConfiguration for RunCmd {
    fn shared_params(&self) -> &SharedParams {
        &self.shared_params
    }

    fn pruning_params(&self) -> Option<&PruningParams> {
        Some(&self.pruning_params)
    }

    fn node_key_params(&self) -> Option<&NodeKeyParams> {
        None
    }

    fn role(&self, _is_dev: bool) -> sc_cli::Result<Role> {
        Ok(Role::Full)
    }

    fn blocks_pruning(&self) -> sc_cli::Result<BlocksPruning> {
        // TODO: configurable blocks pruning
        Ok(BlocksPruning::KeepAll)
    }
}
