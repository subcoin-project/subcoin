//! Helper for answering the subcoin specific requests from a remote peer via the
//! request-response protocol.

use codec::{Decode, Encode};
use futures::channel::oneshot;
use futures::stream::StreamExt;
use sc_client_api::{BlockBackend, HeaderBackend, ProofProvider};
use sc_network::config::ProtocolId;
use sc_network::request_responses::{IncomingRequest, OutgoingResponse};
use sc_network::{NetworkBackend, PeerId, MAX_RESPONSE_SIZE};
use sp_api::ProvideRuntimeApi;
use sp_runtime::traits::Block as BlockT;
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;
use subcoin_primitives::runtime::SubcoinApi;

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

/// Subcoin network specific requests.
#[derive(Debug, codec::Encode, codec::Decode)]
pub enum SubNetworkRequest<Block: BlockT> {
    /// Requests the number of total coins at a specified block.
    GetCoinsCount { block_hash: Block::Hash },
}

/// Subcoin network specific responses.
#[derive(Debug, codec::Encode, codec::Decode)]
pub enum SubNetworkResponse<Block: BlockT> {
    /// Request the number of total coins at the specified block.
    CoinsCount { block_hash: Block::Hash, count: u64 },
}

/// Handler for incoming block requests from a remote peer.
pub struct NetworkRequestHandler<Block, Client> {
    client: Arc<Client>,
    request_receiver: async_channel::Receiver<IncomingRequest>,
    _phantom: PhantomData<Block>,
}

impl<B, Client> NetworkRequestHandler<B, Client>
where
    B: BlockT,
    Client: HeaderBackend<B>
        + BlockBackend<B>
        + ProofProvider<B>
        + ProvideRuntimeApi<B>
        + Send
        + Sync
        + 'static,
    Client::Api: SubcoinApi<B>,
{
    /// Create a new [`NetworkRequestHandler`].
    pub fn new<N: NetworkBackend<B, <B as BlockT>::Hash>>(
        protocol_id: &ProtocolId,
        fork_id: Option<&str>,
        client: Arc<Client>,
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
        let request = SubNetworkRequest::<B>::decode(&mut payload.as_slice())?;

        tracing::debug!(target: LOG_TARGET, "Handling request from {peer:?}: {request:?}");

        let result = match request {
            SubNetworkRequest::GetCoinsCount { block_hash } => {
                let count = self.client.runtime_api().coins_count(block_hash)?;
                let response = SubNetworkResponse::<B>::CoinsCount { block_hash, count };
                Ok(response.encode())
            }
        };

        pending_response
            .send(OutgoingResponse {
                result,
                reputation_changes: Vec::new(),
                sent_feedback: None,
            })
            .map_err(|_| HandleRequestError::SendResponse)
    }
}

#[derive(Debug, thiserror::Error)]
enum HandleRequestError {
    #[error("Failed to decode block hash: {0}.")]
    InvalidHash(#[from] codec::Error),

    #[error(transparent)]
    Client(#[from] sp_blockchain::Error),

    #[error(transparent)]
    RuntimeApi(#[from] sp_api::ApiError),

    #[error("Failed to send response.")]
    SendResponse,
}
