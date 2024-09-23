//! Schema for the cumulative work for each block in the chain.

use bitcoin::consensus::Encodable;
use bitcoin::{BlockHash, Target, Work};
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
    let mut encoded_block_hash = Vec::new();
    bitcoin_block_hash
        .consensus_encode(&mut encoded_block_hash)
        .expect("Encode bitcoin block hash must not fail; qed");
    (b"chain_work", encoded_block_hash).encode()
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

    // `Work` has no exposed API that can be used for encode/decode purpose,
    // thus using `Target` as a workaround to store the inner bytes.
    let encoded_work = chain_work.to_target().to_le_bytes();

    write_aux((key, encoded_work.to_vec()))
}

/// Load the cumulative chain work for given block.
pub(crate) fn load_chain_work<B: AuxStore>(
    backend: &B,
    bitcoin_block_hash: BlockHash,
) -> ClientResult<Work> {
    load_decode(backend, chain_work_key(bitcoin_block_hash).as_slice())?
        .map(|encoded_target| {
            let target = Target::from_le_bytes(encoded_target);
            target.to_work()
        })
        .ok_or(sp_blockchain::Error::Backend(format!(
            "Cumulative chain work for #{bitcoin_block_hash} not found",
        )))
}
