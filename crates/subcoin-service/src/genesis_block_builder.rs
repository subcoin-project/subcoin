use sc_client_api::backend::Backend;
use sc_client_api::BlockImportOperation;
use sc_executor::RuntimeVersionOf;
use sc_service::BuildGenesisBlock;
use sp_core::storage::Storage;
use sp_core::Encode;
use sp_runtime::traits::{Block as BlockT, HashingFor, Header as HeaderT};
use sp_runtime::BuildStorage;
use std::marker::PhantomData;
use std::sync::Arc;

/// Subcoin genesis block builder.
///
/// The genesis state is handled within the pallet-bitcoin, the genesis block data is generated
/// from the corresponding Bitcoin genesis block.
pub struct GenesisBlockBuilder<Block: BlockT, B, E, TransactionAdapter> {
    network: bitcoin::Network,
    genesis_storage: Storage,
    commit_genesis_state: bool,
    backend: Arc<B>,
    executor: E,
    _phantom: PhantomData<(Block, TransactionAdapter)>,
}

impl<Block: BlockT, B, E: Clone, TransactionAdapter> Clone
    for GenesisBlockBuilder<Block, B, E, TransactionAdapter>
{
    fn clone(&self) -> Self {
        Self {
            network: self.network,
            genesis_storage: self.genesis_storage.clone(),
            commit_genesis_state: self.commit_genesis_state,
            backend: self.backend.clone(),
            executor: self.executor.clone(),
            _phantom: Default::default(),
        }
    }
}

impl<Block, B, E, TransactionAdapter> GenesisBlockBuilder<Block, B, E, TransactionAdapter>
where
    Block: BlockT,
    B: Backend<Block>,
    E: RuntimeVersionOf,
    TransactionAdapter: subcoin_primitives::BitcoinTransactionAdapter<Block>,
{
    /// Constructs a new instance of [`GenesisBlockBuilder`].
    pub fn new(
        network: bitcoin::Network,
        build_genesis_storage: &dyn BuildStorage,
        commit_genesis_state: bool,
        backend: Arc<B>,
        executor: E,
    ) -> sp_blockchain::Result<Self> {
        let genesis_storage = build_genesis_storage
            .build_storage()
            .map_err(sp_blockchain::Error::Storage)?;
        Ok(Self {
            network,
            genesis_storage,
            commit_genesis_state,
            backend,
            executor,
            _phantom: Default::default(),
        })
    }
}

fn substrate_genesis_block<Block, TransactionAdapter>(
    network: bitcoin::Network,
    state_root: Block::Hash,
) -> Block
where
    Block: BlockT,
    TransactionAdapter: subcoin_primitives::BitcoinTransactionAdapter<Block>,
{
    let block = bitcoin::constants::genesis_block(network);

    let extrinsics = block
        .txdata
        .clone()
        .into_iter()
        .map(TransactionAdapter::bitcoin_transaction_into_extrinsic)
        .collect::<Vec<_>>();

    let extrinsics_root = frame_system::extrinsics_data_root::<HashingFor<Block>>(
        extrinsics.iter().map(|xt| xt.encode()).collect(),
        sp_core::storage::StateVersion::try_from(subcoin_runtime::VERSION.system_version)
            .expect("Invalid system version"),
    );

    let digest = subcoin_primitives::substrate_header_digest(&block.header);

    let header = <<Block as BlockT>::Header as HeaderT>::new(
        0u32.into(),
        extrinsics_root,
        state_root,
        Default::default(),
        digest,
    );

    Block::new(header, extrinsics)
}

impl<Block, B, E, TransactionAdapter> BuildGenesisBlock<Block>
    for GenesisBlockBuilder<Block, B, E, TransactionAdapter>
where
    Block: BlockT,
    B: Backend<Block>,
    E: RuntimeVersionOf,
    TransactionAdapter: subcoin_primitives::BitcoinTransactionAdapter<Block>,
{
    type BlockImportOperation = <B as Backend<Block>>::BlockImportOperation;

    fn build_genesis_block(self) -> sp_blockchain::Result<(Block, Self::BlockImportOperation)> {
        let Self {
            network,
            genesis_storage,
            commit_genesis_state,
            backend,
            executor,
            _phantom,
        } = self;

        let genesis_state_version = sc_service::resolve_state_version_from_wasm::<
            _,
            HashingFor<Block>,
        >(&genesis_storage, &executor)?;

        let mut op = backend.begin_operation()?;
        let state_root =
            op.set_genesis_state(genesis_storage, commit_genesis_state, genesis_state_version)?;

        let genesis_block =
            substrate_genesis_block::<Block, TransactionAdapter>(network, state_root);

        Ok((genesis_block, op))
    }
}
