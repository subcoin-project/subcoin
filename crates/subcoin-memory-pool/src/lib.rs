//! # Bitcoin Mempool Overview
//!
//! 1. Transaction Validation.
//!     - Transactions are validated before being added to the mempool.
//!     - Validation includes checking transaction size, fees and script validity.
//! 2. Fee Management
//!     - Transactions are prioritized based on their fee rate.
//!     - The mempool may evict lower-fee transactions if it reaches its size limit.
//! 3. Ancestors and Descendants.
//!     - The mempool tracks transaction dependencies to ensure that transactions are minded the
//!     correct order.

mod options;
mod policy;

use self::options::MemPoolOptions;
use self::policy::{is_standard_tx, StandardTxError};
use bitcoin::Transaction;
use sc_client_api::HeaderBackend;
use sp_runtime::traits::Block as BlockT;
use std::marker::PhantomData;
use std::sync::Arc;
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
    pub fn accept_single_transaction(&self, tx: Transaction) -> Result<(), Error> {
        self.pre_checks(&tx)?;

        // Fee checks

        // Script validation.

        // ReplacementChecks

        // PolicyScriptchecks.

        // ConsensusScriptChecks

        // Ancestor/descendent limits.

        // Add to pool
        Ok(())
    }

    // validation.cpp MemPoolAccept::PreChecks()
    fn pre_checks(&self, tx: &Transaction) -> Result<(), Error> {
        check_transaction_sanity(tx)?;

        // Coinbase is only valid in a block, not as a loose transaction.
        if tx.is_coinbase() {
            return Err(Error::Coinbase);
        }

        if self.options.require_standard {
            is_standard_tx(
                &tx,
                options::MAX_OP_RETURN_RELAY,
                self.options.permit_bare_multisig,
                self.options.dust_relay_feerate,
            )
            .map_err(Error::NotStandard)?;
        }

        // TODO: CheckFinalTxAtTip

        // TODO: Check the existence txid, wtxid

        // TODO: do all inputs exist?

        // TODO: CheckTxInputs()

        Ok(())
    }

    pub fn remove_transaction(&self, tx: Transaction) -> Result<(), Error> {
        Ok(())
    }

    pub fn trim_to_size(&self, max_size: usize) {}
}
