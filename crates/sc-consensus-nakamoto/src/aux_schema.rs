//! Schema for the cumulative work for each block in the chain.

use bitcoin::hashes::Hash;
use bitcoin::{BlockHash, Work};
use codec::{Decode, Encode};
use sc_client_api::backend::AuxStore;
use sp_blockchain::{Error as ClientError, Result as ClientResult};

fn load_decode<B, T>(backend: &B, key: &[u8]) -> ClientResult<Option<T>>
where
    B: AuxStore,
    T: Decode,
{
    match backend.get_aux(key)? {
        Some(t) => T::decode(&mut &t[..]).map(Some).map_err(|e: codec::Error| {
            ClientError::Backend(format!("Subcoin DB is corrupted. Decode error: {e}"))
        }),
        None => Ok(None),
    }
}

/// The aux storage key used to store the block weight of the given block hash.
fn chain_work_key(bitcoin_block_hash: BlockHash) -> Vec<u8> {
    (b"chain_work", bitcoin_block_hash.as_byte_array().to_vec()).encode()
}

/// Write the cumulative chain work of a block to aux storage.
pub(crate) fn write_chain_work<F, R>(
    bitcoin_block_hash: BlockHash,
    chain_work: Work,
    write_aux: F,
) -> R
where
    F: FnOnce((Vec<u8>, Vec<u8>)) -> R,
{
    let key = chain_work_key(bitcoin_block_hash);

    write_aux((key, chain_work.to_le_bytes().to_vec()))
}

/// Load the cumulative chain work for given block.
pub(crate) fn load_chain_work<B: AuxStore>(
    backend: &B,
    bitcoin_block_hash: BlockHash,
) -> ClientResult<Work> {
    load_decode(backend, chain_work_key(bitcoin_block_hash).as_slice())?
        .map(Work::from_le_bytes)
        .ok_or(sp_blockchain::Error::Backend(format!(
            "Cumulative chain work for #{bitcoin_block_hash} not found",
        )))
}
