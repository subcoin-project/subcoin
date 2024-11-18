use crate::commands::blockchain::{ClientParams, MergedParams};
use sc_client_api::HeaderBackend;
use std::sync::Arc;
use subcoin_primitives::BitcoinTransactionAdapter;
use subcoin_service::FullClient;

#[derive(Debug, clap::Parser)]
pub struct ParseBlock {
    /// Specify the number of block to dump.
    ///
    /// Defaults to the best block.
    #[clap(long)]
    height: Option<u32>,

    #[allow(missing_docs)]
    #[clap(flatten)]
    client_params: ClientParams,
}

pub struct ParseBlockCmd {
    height: Option<u32>,
    pub(super) params: MergedParams,
}

impl ParseBlockCmd {
    pub fn execute(self, client: Arc<FullClient>) -> sc_cli::Result<()> {
        let block_number = self.height.unwrap_or_else(|| client.info().best_number);

        let block_hash = client.hash(block_number)?.ok_or_else(|| {
            sp_blockchain::Error::Backend(format!("Hash for {block_number} not found"))
        })?;

        let block_body = client.body(block_hash)?.ok_or_else(|| {
            sp_blockchain::Error::Backend(format!(
                "Body for #{block_number},{block_hash} not found"
            ))
        })?;

        let txdata = block_body
            .iter()
            .map(|xt| {
                <subcoin_service::TransactionAdapter as BitcoinTransactionAdapter<
                    subcoin_runtime::interface::OpaqueBlock,
                >>::extrinsic_to_bitcoin_transaction(xt)
            })
            .collect::<Vec<_>>();

        let mut num_op_return = 0;
        for (i, tx) in txdata.into_iter().enumerate() {
            for (j, output) in tx.output.into_iter().enumerate() {
                let is_op_return = output.script_pubkey.is_op_return();
                println!("{i}:{j}: {is_op_return:?}");

                if is_op_return {
                    num_op_return += 1;
                }
            }
        }

        println!("There are {num_op_return} OP_RETURN in block #{block_number},{block_hash}");

        Ok(())
    }
}

impl From<ParseBlock> for ParseBlockCmd {
    fn from(parse_block: ParseBlock) -> Self {
        let ParseBlock {
            height,
            client_params,
        } = parse_block;
        Self {
            height,
            params: client_params.into_merged_params(),
        }
    }
}
