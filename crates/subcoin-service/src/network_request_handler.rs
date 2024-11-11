//! Helper for handling (i.e. answering) subcoin specific requests from a remote peer via the
//! `crate::request_responses::RequestResponsesBehaviour`.

use codec::{Decode, Encode};
use futures::channel::oneshot;
use futures::stream::StreamExt;
use sc_client_api::{BlockBackend, HeaderBackend, ProofProvider};
use sc_network::config::ProtocolId;
use sc_network::request_responses::{IncomingRequest, OutgoingResponse};
use sc_network::{NetworkBackend, PeerId, MAX_RESPONSE_SIZE};
use sp_runtime::traits::Block as BlockT;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, trace};

const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024; // Actual reponse may be bigger.
const MAX_NUMBER_OF_SAME_REQUESTS_PER_PEER: usize = 2;

const LOG_TARGET: &str = "sync";

mod rep {
    use sc_network::ReputationChange as Rep;

    /// Reputation change when a peer sent us the same request multiple times.
    pub const SAME_REQUEST: Rep = Rep::new(i32::MIN, "Same state request multiple times");
}

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
    if let Some(fork_id) = fork_id {
        format!(
            "/{}/{}/subcoin/1",
            array_bytes::bytes2hex("", genesis_hash),
            fork_id
        )
    } else {
        format!("/{}/subcoin/1", array_bytes::bytes2hex("", genesis_hash))
    }
}

/// Generate the legacy state protocol name from chain specific protocol identifier.
fn generate_legacy_protocol_name(protocol_id: &ProtocolId) -> String {
    format!("/{}/subcoin/1", protocol_id.as_ref())
}

/// Subcoin network specific requests via request-response.
#[derive(Debug, codec::Encode, codec::Decode)]
pub enum SubNetworkRequest<Block: BlockT> {
    /// Requests the count of key-value pairs in the state at a specified block.
    GetStateKeyValueCount { block_hash: Block::Hash },
}

/// Subcoin network specific responses via request-response.
#[derive(Debug, codec::Encode, codec::Decode)]
pub enum SubNetworkResponse<Block: BlockT> {
    /// Request the count of key values of the state at the specified block.
    StateKeyValueCount { block_hash: Block::Hash, count: u64 },
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
    Client: HeaderBackend<B> + BlockBackend<B> + ProofProvider<B> + Send + Sync + 'static,
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
                    debug!(target: LOG_TARGET, "========== Handled subcoin request from {peer}")
                }
                Err(e) => {
                    debug!(target: LOG_TARGET, "========== Failed to handle subcoin request from {peer}: {e:?}")
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

        tracing::info!(target: LOG_TARGET, "============== Handling request from {peer:?}: {request:?}");

        let result = match request {
            SubNetworkRequest::GetStateKeyValueCount { block_hash } => {
                let response = SubNetworkResponse::<B>::StateKeyValueCount {
                    block_hash,
                    count: 888,
                };

                tracing::info!(
                    "==================== Sending response: {:?}",
                    response.encode()
                );

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

    #[error("Failed to send response.")]
    SendResponse,

    #[error("Failed to compress response: {0}.")]
    Compress(std::io::Error),
}
