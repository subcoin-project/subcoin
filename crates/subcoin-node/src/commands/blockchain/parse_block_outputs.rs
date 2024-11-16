use crate::commands::blockchain::{ClientParams, MergedParams};
use sc_client_api::HeaderBackend;
use std::sync::Arc;
use subcoin_primitives::BitcoinTransactionAdapter;
use subcoin_service::FullClient;

#[derive(Debug, clap::Parser)]
pub struct ParseBlockOutputs {
    /// Specify the number of block to dump.
    ///
    /// Defaults to the best block.
    #[clap(long)]
    height: Option<u32>,

    #[allow(missing_docs)]
    #[clap(flatten)]
    client_params: ClientParams,
}

pub struct ParseBlockOutputsCmd {
    height: Option<u32>,
    pub(super) params: MergedParams,
}

impl ParseBlockOutputsCmd {
    pub fn execute(self, client: Arc<FullClient>) -> sc_cli::Result<()> {
        let block_number = self.height.unwrap_or_else(|| client.info().best_number);
        let block_hash = client.hash(block_number)?.unwrap();
        let block_body = client.body(block_hash)?.unwrap();
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
        println!("There are {num_op_return} OP_RETURN in block #{block_number}");
        Ok(())
    }
}

impl From<ParseBlockOutputs> for ParseBlockOutputsCmd {
    fn from(parse_block_outputs: ParseBlockOutputs) -> Self {
        let ParseBlockOutputs {
            height,
            client_params,
        } = parse_block_outputs;
        Self {
            height,
            params: client_params.into_merged_params(),
        }
    }
}
