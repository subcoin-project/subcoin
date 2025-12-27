//! Helper for answering the subcoin specific requests from a remote peer via the
//! request-response protocol.

use codec::{Decode, Encode};
use futures::channel::oneshot;
use futures::stream::StreamExt;
use sc_client_api::{BlockBackend, HeaderBackend, ProofProvider};
use sc_network::config::ProtocolId;
use sc_network::request_responses::{IncomingRequest, OutgoingResponse};
use sc_network::{MAX_RESPONSE_SIZE, NetworkBackend, PeerId};
use sp_runtime::traits::Block as BlockT;
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;

const LOG_TARGET: &str = "sync::subcoin";

/// Generates a `RequestResponseProtocolConfig` for the state request protocol, refusing incoming
/// requests.
pub fn generate_protocol_config<
    Hash: AsRef<[u8]>,
    B: BlockT,
    N: NetworkBackend<B, <B as BlockT>::Hash>,
>(
    protocol_id: &ProtocolId,
    genesis_hash: Hash,
    fork_id: Option<&str>,
    inbound_queue: async_channel::Sender<IncomingRequest>,
) -> N::RequestResponseProtocolConfig {
    N::request_response_config(
        generate_protocol_name(genesis_hash, fork_id).into(),
        std::iter::once(generate_legacy_protocol_name(protocol_id).into()).collect(),
        1024 * 1024,
        MAX_RESPONSE_SIZE,
        Duration::from_secs(40),
        Some(inbound_queue),
    )
}

/// Generate the state protocol name from the genesis hash and fork id.
fn generate_protocol_name<Hash: AsRef<[u8]>>(genesis_hash: Hash, fork_id: Option<&str>) -> String {
    let genesis_hash = genesis_hash.as_ref();
    let genesis_hash = array_bytes::bytes2hex("", genesis_hash);
    if let Some(fork_id) = fork_id {
        format!("/{genesis_hash}/{fork_id}/subcoin/1",)
    } else {
        format!("/{genesis_hash}/subcoin/1")
    }
}

/// Generate the legacy state protocol name from chain specific protocol identifier.
fn generate_legacy_protocol_name(protocol_id: &ProtocolId) -> String {
    format!("/{}/subcoin/1", protocol_id.as_ref())
}

/// Versioned network requests.
#[derive(Debug, codec::Encode, codec::Decode)]
pub enum VersionedNetworkRequest<Block: BlockT> {
    V1(v1::NetworkRequest<Block>),
}

/// Versioned network responses.
#[derive(Debug, codec::Encode, codec::Decode)]
pub enum VersionedNetworkResponse<Block: BlockT> {
    V1(Result<v1::NetworkResponse<Block>, String>),
}

pub mod v1 {
    use sp_runtime::traits::{Block as BlockT, NumberFor};

    /// MuHash commitment for UTXO set verification (768 bytes serialized).
    #[derive(Debug, Clone, codec::Encode, codec::Decode)]
    pub struct MuHashCommitment(pub [u8; 768]);

    impl MuHashCommitment {
        /// Create from raw bytes.
        pub fn from_bytes(bytes: [u8; 768]) -> Self {
            Self(bytes)
        }

        /// Get the inner bytes.
        pub fn as_bytes(&self) -> &[u8; 768] {
            &self.0
        }

        /// Compute the txoutset muhash (32-byte hash in Bitcoin Core format).
        ///
        /// This deserializes the 768-byte state and computes the final hash.
        pub fn txoutset_muhash(&self) -> String {
            let muhash = subcoin_crypto::MuHash3072::deserialize(&self.0);
            muhash.txoutset_muhash()
        }
    }

    /// Subcoin network specific requests.
    #[derive(Debug, codec::Encode, codec::Decode)]
    pub enum NetworkRequest<Block: BlockT> {
        /// Requests the best block.
        GetBestBlock,
        /// Requests the number of total coins at a specified block.
        GetCoinsCount { block_hash: Block::Hash },
        /// Request the header of specified block.
        GetBlockHeader { block_number: NumberFor<Block> },
        /// Request UTXO set info for snap sync.
        GetUtxoSetInfo,
        /// Request a chunk of UTXOs for snap sync.
        GetUtxoChunk {
            /// Cursor for pagination (36 bytes: txid + vout).
            /// None means start from beginning.
            cursor: Option<[u8; 36]>,
            /// Maximum number of UTXOs to return.
            max_entries: u32,
        },
    }

    /// Subcoin network specific responses.
    #[derive(Debug, codec::Encode, codec::Decode)]
    pub enum NetworkResponse<Block: BlockT> {
        BestBlock {
            best_hash: Block::Hash,
            best_number: NumberFor<Block>,
        },
        /// The number of total coins at the specified block.
        CoinsCount { block_hash: Block::Hash, count: u64 },
        /// Block header.
        BlockHeader { block_header: Block::Header },
        /// UTXO set info for snap sync.
        UtxoSetInfo {
            /// Block height of the UTXO set.
            height: u32,
            /// Total number of UTXOs.
            utxo_count: u64,
            /// MuHash commitment.
            muhash: MuHashCommitment,
        },
        /// A chunk of UTXOs for snap sync.
        UtxoChunk {
            /// UTXO entries in this chunk.
            entries: Vec<UtxoEntry>,
            /// Cursor for next chunk (None if this is the last chunk).
            next_cursor: Option<[u8; 36]>,
            /// Whether this is the final chunk.
            is_complete: bool,
        },
    }

    /// A single UTXO entry for P2P transmission.
    #[derive(Debug, Clone, codec::Encode, codec::Decode)]
    pub struct UtxoEntry {
        /// Transaction ID (32 bytes).
        pub txid: [u8; 32],
        /// Output index.
        pub vout: u32,
        /// Whether from coinbase transaction.
        pub is_coinbase: bool,
        /// Value in satoshis.
        pub amount: u64,
        /// Block height where the UTXO was created.
        pub height: u32,
        /// ScriptPubKey.
        pub script_pubkey: Vec<u8>,
    }
}

/// Handler for incoming block requests from a remote peer.
pub struct NetworkRequestHandler<Block, Client> {
    client: Arc<Client>,
    bitcoin_state: Arc<subcoin_bitcoin_state::BitcoinState>,
    request_receiver: async_channel::Receiver<IncomingRequest>,
    _phantom: PhantomData<Block>,
}

impl<B, Client> NetworkRequestHandler<B, Client>
where
    B: BlockT,
    Client: HeaderBackend<B> + BlockBackend<B> + ProofProvider<B> + Send + Sync + 'static,
{
    /// Create a new [`NetworkRequestHandler`].
    pub fn new<N: NetworkBackend<B, <B as BlockT>::Hash>>(
        protocol_id: &ProtocolId,
        fork_id: Option<&str>,
        client: Arc<Client>,
        bitcoin_state: Arc<subcoin_bitcoin_state::BitcoinState>,
        num_peer_hint: usize,
    ) -> (Self, N::RequestResponseProtocolConfig) {
        // Reserve enough request slots for one request per peer when we are at the maximum
        // number of peers.
        let capacity = std::cmp::max(num_peer_hint, 1);
        let (tx, request_receiver) = async_channel::bounded(capacity);

        let protocol_config = generate_protocol_config::<_, B, N>(
            protocol_id,
            client.info().genesis_hash,
            fork_id,
            tx,
        );

        (
            Self {
                client,
                bitcoin_state,
                request_receiver,
                _phantom: Default::default(),
            },
            protocol_config,
        )
    }

    /// Run [`NetworkRequestHandler`].
    pub async fn run(mut self) {
        while let Some(request) = self.request_receiver.next().await {
            let IncomingRequest {
                peer,
                payload,
                pending_response,
            } = request;

            match self.handle_request(payload, pending_response, &peer) {
                Ok(()) => {
                    tracing::debug!(target: LOG_TARGET, "Handled subcoin request from {peer}")
                }
                Err(e) => {
                    tracing::debug!(target: LOG_TARGET, "Failed to handle subcoin request from {peer}: {e:?}")
                }
            }
        }
    }

    fn handle_request(
        &mut self,
        payload: Vec<u8>,
        pending_response: oneshot::Sender<OutgoingResponse>,
        peer: &PeerId,
    ) -> Result<(), HandleRequestError> {
        let request = VersionedNetworkRequest::<B>::decode(&mut payload.as_slice())?;

        tracing::debug!(target: LOG_TARGET, "Handling request from {peer:?}: {request:?}");

        let response: VersionedNetworkResponse<B> = match request {
            VersionedNetworkRequest::V1(request) => VersionedNetworkResponse::V1(
                self.process_request_v1(request)
                    .map_err(|err| err.to_string()),
            ),
        };

        pending_response
            .send(OutgoingResponse {
                result: Ok(response.encode()),
                reputation_changes: Vec::new(),
                sent_feedback: None,
            })
            .map_err(|_| HandleRequestError::SendResponse)
    }

    fn process_request_v1(
        &self,
        request: v1::NetworkRequest<B>,
    ) -> Result<v1::NetworkResponse<B>, HandleRequestError> {
        match request {
            v1::NetworkRequest::GetBestBlock => {
                let info = self.client.info();
                let response = v1::NetworkResponse::<B>::BestBlock {
                    best_hash: info.best_hash,
                    best_number: info.best_number,
                };
                Ok(response)
            }
            v1::NetworkRequest::GetCoinsCount { block_hash } => {
                // TODO: Wire NativeUtxoStorage through to get actual UTXO count
                // For now, return an error indicating the feature is not available
                Err(HandleRequestError::Client(sp_blockchain::Error::Backend(
                    format!(
                        "GetCoinsCount at {block_hash} not yet implemented with native storage"
                    ),
                )))
            }
            v1::NetworkRequest::GetBlockHeader { block_number } => {
                let block_hash = self.client.hash(block_number)?.ok_or_else(|| {
                    sp_blockchain::Error::Backend(format!(
                        "Hash for block #{block_number} not found"
                    ))
                })?;

                Ok(self
                    .client
                    .header(block_hash)?
                    .map(|block_header| v1::NetworkResponse::<B>::BlockHeader { block_header })
                    .ok_or_else(|| {
                        sp_blockchain::Error::Backend(format!(
                            "Header for #{block_number},{block_hash} not found"
                        ))
                    })?)
            }
            v1::NetworkRequest::GetUtxoSetInfo => {
                let height = self.bitcoin_state.height();
                let utxo_count = self.bitcoin_state.utxo_count();
                let muhash =
                    v1::MuHashCommitment::from_bytes(self.bitcoin_state.muhash_serialized());

                Ok(v1::NetworkResponse::<B>::UtxoSetInfo {
                    height,
                    utxo_count,
                    muhash,
                })
            }
            v1::NetworkRequest::GetUtxoChunk {
                cursor,
                max_entries,
            } => {
                // Debug: verify actual DB entries match metadata on first request
                if cursor.is_none() {
                    if let Err(e) = self.bitcoin_state.count_actual_utxos() {
                        tracing::warn!("Failed to count UTXOs: {e:?}");
                    }
                }

                tracing::info!(
                    "GetUtxoChunk request: cursor={}, max_entries={max_entries}",
                    if cursor.is_some() {
                        "Some(...)"
                    } else {
                        "None"
                    }
                );

                let (utxos, next_cursor, is_complete) = self
                    .bitcoin_state
                    .export_chunk(cursor.as_ref(), max_entries)
                    .map_err(|e| {
                        HandleRequestError::Client(sp_blockchain::Error::Backend(format!(
                            "Failed to export UTXO chunk: {e}"
                        )))
                    })?;

                tracing::info!(
                    "GetUtxoChunk response: entries={}, next_cursor={}, is_complete={is_complete}",
                    utxos.len(),
                    if next_cursor.is_some() {
                        "Some(...)"
                    } else {
                        "None"
                    }
                );

                let entries = utxos
                    .into_iter()
                    .map(|(outpoint, coin)| v1::UtxoEntry {
                        txid: *outpoint.txid.as_ref(),
                        vout: outpoint.vout,
                        is_coinbase: coin.is_coinbase,
                        amount: coin.amount,
                        height: coin.height,
                        script_pubkey: coin.script_pubkey,
                    })
                    .collect();

                Ok(v1::NetworkResponse::<B>::UtxoChunk {
                    entries,
                    next_cursor,
                    is_complete,
                })
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum HandleRequestError {
    #[error("Failed to decode block hash: {0}.")]
    InvalidHash(#[from] codec::Error),

    #[error(transparent)]
    Client(#[from] sp_blockchain::Error),

    #[error("Failed to send response.")]
    SendResponse,
}
