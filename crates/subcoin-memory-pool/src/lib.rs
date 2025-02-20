mod options;
mod policy;

use self::options::MemPoolOptions;
use self::policy::{is_standard_tx, StandardTxError};
use bitcoin::Transaction;
use sc_client_api::HeaderBackend;
use sp_runtime::traits::Block as BlockT;
use std::{marker::PhantomData, sync::Arc};
use subcoin_primitives::consensus::check_transaction_sanity;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Transaction is an individual coinbase")]
    Coinbase,
    #[error("not a standard tx: {0:?}")]
    NotStandard(policy::StandardTxError),
    #[error(transparent)]
    PreliminaryCheck(#[from] subcoin_primitives::consensus::TxError),
}

#[derive(Debug)]
pub struct MemPool<Block, Client> {
    client: Arc<Client>,
    options: MemPoolOptions,
    _phantom: PhantomData<Block>,
}

impl<Block, Client> MemPool<Block, Client>
where
    Block: BlockT,
    Client: HeaderBackend<Block>,
{
    pub fn accept_to_memory_pool(&self, tx: Transaction) -> Result<(), Error> {
        check_transaction_sanity(&tx)?;

        // Coinbase is only valid in a block, not as a loose transaction.
        if tx.is_coinbase() {
            return Err(Error::Coinbase);
        }

        if self.options.require_standard {
            is_standard_tx(
                &tx,
                options::MAX_OP_RETURN_RELAY,
                self.options.permit_bare_multisig,
                self.options.dust_relay_fee,
            )
            .map_err(Error::NotStandard)?;
        }

        if tx.base_size() < policy::MIN_STANDARD_TX_NONWITNESS_SIZE {
            return Err(Error::NotStandard(StandardTxError::TxSizeTooSmall));
        }

        // TODO: CheckFinalTxAtTip

        // TODO: Check the existence txid, wtxid

        // TODO: do all inputs exist?

        // TODO: CheckTxInputs()

        // Fee checks

        // Script validation.

        // Policy checks.

        // Ancestor/descendent limits.

        // Add to pool
        Ok(())
    }
}
