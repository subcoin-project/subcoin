use crate::chain_params::ChainParams;
use crate::verification::HeaderVerifier;
use sc_client_api::{AuxStore, HeaderBackend};
use sc_consensus::{BlockImportParams, Verifier};
use sp_runtime::traits::{Block as BlockT, Header as HeaderT};
use sp_runtime::SaturatedConversion;
use std::sync::Arc;

/// Verifier used by the Substrate import queue.
///
/// Verifies the blocks received from the Substrate networking.
pub struct SubstrateImportQueueVerifier<Block, Client> {
    client: Arc<Client>,
    btc_header_verifier: HeaderVerifier<Block, Client>,
}

impl<Block, Client> SubstrateImportQueueVerifier<Block, Client> {
    /// Constructs a new instance of [`SubstrateImportQueueVerifier`].
    pub fn new(client: Arc<Client>, network: bitcoin::Network) -> Self {
        Self {
            client: client.clone(),
            btc_header_verifier: HeaderVerifier::new(client, ChainParams::new(network)),
        }
    }
}

#[async_trait::async_trait]
impl<Block, Client> Verifier<Block> for SubstrateImportQueueVerifier<Block, Client>
where
    Block: BlockT,
    Client: HeaderBackend<Block> + AuxStore,
{
    async fn verify(
        &self,
        mut block_import_params: BlockImportParams<Block>,
    ) -> Result<BlockImportParams<Block>, String> {
        let substrate_header = &block_import_params.header;

        let btc_header =
            subcoin_primitives::extract_bitcoin_block_header::<Block>(substrate_header)
                .map_err(|err| format!("Failed to extract bitcoin header: {err:?}"))?;

        self.btc_header_verifier
            .verify(&btc_header)
            .map_err(|err| format!("Invalid header: {err:?}"))?;

        let (chain_work, fork_choice) = crate::block_import::calculate_chain_work_and_fork_choice(
            &self.client,
            &btc_header,
            (*substrate_header.number()).saturated_into::<u32>(),
        )
        .map_err(|err| format!("Failed to calculate cumulative work: {err:?}"))?;

        block_import_params.fork_choice = Some(fork_choice);

        let bitcoin_block_hash =
            subcoin_primitives::extract_bitcoin_block_hash::<Block>(substrate_header)
                .map_err(|err| format!("Failed to extract bitcoin block hash: {err:?}"))?;

        let substrate_block_hash = substrate_header.hash();

        crate::block_import::write_aux_storage::<Block>(
            &mut block_import_params,
            bitcoin_block_hash,
            substrate_block_hash,
            chain_work,
        );

        Ok(block_import_params)
    }
}
