use crate::worker::Event;
use crate::{Bandwidth, Error, PeerId};
use bitcoin::consensus::{encode, Decodable, Encodable};
use bitcoin::p2p::message::{NetworkMessage, RawNetworkMessage, MAX_MSG_SIZE};
use futures::FutureExt;
use sc_service::SpawnTaskHandle;
use std::collections::VecDeque;
use std::fmt::Debug;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

const MSG_HEADER_SIZE: usize = 24;

/// Channel for sending messages to the peer.
pub type ConnectionWriter = UnboundedSender<NetworkMessage>;

/// Represents the direction of connection.
///
/// This enum is used to distinguish between inbound and outbound connections.
/// An `Inbound` connection indicates that the peer initiated the connection
/// to our node, while an `Outbound` connection indicates that our node initiated
/// the connection to the peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Connection initiated by the remote node.
    Inbound,

    /// Connection initiated by the local node.
    Outbound,
}

impl Direction {
    pub fn is_inbound(&self) -> bool {
        matches!(self, Self::Inbound)
    }

    pub fn is_outbound(&self) -> bool {
        matches!(self, Self::Outbound)
    }
}

/// An incoming peer connection.
#[derive(Debug)]
pub struct NewConnection {
    pub peer_addr: PeerId,
    pub local_addr: PeerId,
    pub direction: Direction,
    pub writer: ConnectionWriter,
    pub disconnect_signal: Arc<AtomicBool>,
}

/// Message stream decoder.
///
/// Used to turn a byte stream into network messages.
#[derive(Debug)]
struct NetworkMessageDecoder {
    unparsed: Vec<u8>,
}

impl NetworkMessageDecoder {
    /// Constructs a new [`NetworkMessageDecoder`].
    fn new(capacity: usize) -> Self {
        Self {
            unparsed: Vec::with_capacity(capacity),
        }
    }

    /// Input bytes into the decoder.
    fn input(&mut self, bytes: &[u8]) {
        self.unparsed.extend_from_slice(bytes);
    }

    /// Decode and return the next message.
    ///
    /// Returns [`None`] if nothing was decoded.
    fn decode_next<D: Decodable>(&mut self) -> Result<Option<D>, encode::Error> {
        match encode::deserialize_partial::<D>(&self.unparsed) {
            Ok((msg, index)) => {
                // Drain deserialized bytes only.
                self.unparsed.drain(..index);
                Ok(Some(msg))
            }

            Err(encode::Error::Io(err)) if err.kind() == bitcoin::io::ErrorKind::UnexpectedEof => {
                Ok(None)
            }
            Err(err) => Err(err),
        }
    }
}

#[derive(Clone)]
pub struct ConnectionInitiator {
    network: bitcoin::Network,
    network_event_sender: UnboundedSender<Event>,
    spawn_handle: SpawnTaskHandle,
    bandwidth: Bandwidth,
    ipv4_only: bool,
}

impl ConnectionInitiator {
    /// Timeout for the stream connection in seconds.
    const CONNECT_TIMEOUT: u64 = 5;

    /// Constructs a new instance of [`ConnectionInitiator`].
    pub fn new(
        network: bitcoin::Network,
        network_event_sender: UnboundedSender<Event>,
        spawn_handle: SpawnTaskHandle,
        bandwidth: Bandwidth,
        ipv4_only: bool,
    ) -> Self {
        Self {
            network,
            network_event_sender,
            spawn_handle,
            bandwidth,
            ipv4_only,
        }
    }

    /// Makes a new outbound connection in the background.
    pub fn initiate_outbound_connection(&self, addr: PeerId) {
        self.spawn_handle.spawn("outbound-connection", None, {
            let connection_initiator = self.clone();

            async move {
                let outbound_connection_fut = async {
                    let stream_result = tokio::time::timeout(
                        Duration::from_secs(Self::CONNECT_TIMEOUT),
                        TcpStream::connect(addr),
                    )
                    .await
                    .map_err(|_| Error::ConnectionTimeout)?;
                    let stream = stream_result?;
                    connection_initiator.initiate_new_connection(Direction::Outbound, stream)
                };

                if let Err(err) = outbound_connection_fut.await {
                    let _ = connection_initiator.network_event_sender.send(
                        Event::OutboundConnectionFailure {
                            peer_addr: addr,
                            reason: err,
                        },
                    );
                }
            }
        });
    }

    /// Makes a new inbound connection.
    pub fn initiate_inbound_connection(&self, stream: TcpStream) -> Result<(), Error> {
        self.initiate_new_connection(Direction::Inbound, stream)
    }

    fn initiate_new_connection(
        &self,
        direction: Direction,
        stream: TcpStream,
    ) -> Result<(), Error> {
        let peer_addr = stream.peer_addr()?;

        if self.ipv4_only && peer_addr.is_ipv6() {
            return Err(Error::Ipv4Only);
        }

        let local_addr = stream.local_addr()?;

        tracing::debug!(?peer_addr, ?local_addr, ?direction, "Opened new connection");

        let (network_message_sender, network_message_receiver) = unbounded_channel();

        let disconnect_signal = Arc::new(AtomicBool::new(false));

        self.network_event_sender
            .send(Event::NewConnection(NewConnection {
                peer_addr,
                local_addr,
                direction,
                writer: network_message_sender,
                disconnect_signal: disconnect_signal.clone(),
            }))
            .map_err(|_| Error::NetworkEventStreamError)?;

        // Maintain the communication with a remote peer over the given socket endlessly.
        let (readable, writable) = stream.into_split();

        self.spawn_handle.spawn(
            "connection-reader",
            None,
            {
                let bandwidth = self.bandwidth.clone();
                let network_event_sender = self.network_event_sender.clone();
                let disconnect_signal = disconnect_signal.clone();

                async move {
                    if let Err(err) = read_peer_messages(
                        peer_addr,
                        direction,
                        readable,
                        network_event_sender.clone(),
                        disconnect_signal,
                        bandwidth,
                    )
                    .await
                    {
                        let _ = network_event_sender.send(Event::Disconnect {
                            peer_addr,
                            reason: err,
                        });
                    }
                }
            }
            .boxed(),
        );

        self.spawn_handle.spawn("connection-writer", None, {
            let bandwidth = self.bandwidth.clone();
            let network_event_sender = self.network_event_sender.clone();
            let network = self.network;

            async move {
                if let Err(err) = send_peer_messages(
                    peer_addr,
                    network,
                    writable,
                    network_message_receiver,
                    disconnect_signal,
                    bandwidth,
                )
                .await
                {
                    let _ = network_event_sender.send(Event::Disconnect {
                        peer_addr,
                        reason: err,
                    });
                }
            }
            .boxed()
        });

        Ok(())
    }
}

async fn read_peer_messages(
    peer: PeerId,
    direction: Direction,
    readable: tokio::net::tcp::OwnedReadHalf,
    network_event_sender: UnboundedSender<Event>,
    disconnect_signal: Arc<AtomicBool>,
    bandwidth: Bandwidth,
) -> Result<(), Error> {
    let mut decoder = NetworkMessageDecoder::new(1024 * 192);

    loop {
        if disconnect_signal.load(Ordering::SeqCst) {
            tracing::trace!(?peer, "Stopping the reader task");
            return Ok(());
        }

        readable.readable().await?;

        // TODO: optimize this, no need to always allocate the maximum buffer.
        let mut read_buffer = vec![0; MAX_MSG_SIZE];

        // Try to read data, this may still fail with `WouldBlock`
        // if the readiness event is a false positive.
        match readable.as_ref().try_read(&mut read_buffer) {
            Ok(0) => {
                tracing::trace!(from = ?peer, "<= recv 0 bytes");
                return Err(Error::PeerShutdown);
            }
            Ok(n) => {
                tracing::trace!(from = ?peer, "<= recv {n} bytes");

                bandwidth
                    .total_bytes_inbound
                    .fetch_add(n as u64, Ordering::Relaxed);

                decoder.input(&read_buffer[..n]);

                while let Some(msg) = decoder.decode_next::<RawNetworkMessage>()? {
                    network_event_sender
                        .send(Event::PeerMessage {
                            from: peer,
                            direction,
                            payload: msg.into_payload(),
                        })
                        .map_err(|_| Error::NetworkEventStreamError)?;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                continue;
            }
            Err(e) => {
                return Err(e.into());
            }
        }
    }
}

async fn send_peer_messages(
    peer: PeerId,
    network: bitcoin::Network,
    writable: tokio::net::tcp::OwnedWriteHalf,
    mut network_message_receiver: UnboundedReceiver<NetworkMessage>,
    disconnect_signal: Arc<AtomicBool>,
    bandwidth: Bandwidth,
) -> Result<(), Error> {
    let magic = network.magic();

    // Cache the messages yet to be processed due to the false positive readiness.
    let mut msg_buffer: VecDeque<(&'static str, Vec<u8>)> = VecDeque::with_capacity(32);

    loop {
        if disconnect_signal.load(Ordering::SeqCst) {
            tracing::trace!(?peer, "Stopping the writer task");
            return Ok(());
        }

        writable.writable().await?;

        let mut buffered_front_sent = false;

        if let Some((cmd, msg)) = msg_buffer.front() {
            // Try to write data, this may still fail with `WouldBlock`
            // if the readiness event is a false positive.
            match writable.as_ref().try_write(msg) {
                Ok(n) => {
                    bandwidth
                        .total_bytes_outbound
                        .fetch_add(n as u64, Ordering::Relaxed);

                    let msg_len = msg.len().saturating_sub(MSG_HEADER_SIZE);
                    tracing::trace!(to = ?peer, "=> {cmd} ({msg_len} bytes) sent successfully");
                    buffered_front_sent = true;
                }
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::WouldBlock {
                        continue;
                    } else {
                        return Err(e.into());
                    }
                }
            };
        }

        if buffered_front_sent {
            msg_buffer.pop_front();
            continue;
        }

        if let Some(network_message) = network_message_receiver.recv().await {
            tracing::trace!(to = ?peer, "Sending {network_message:?}");

            let raw_network_message = RawNetworkMessage::new(magic, network_message);

            let mut msg = Vec::with_capacity(MAX_MSG_SIZE);
            raw_network_message.consensus_encode(&mut msg)?;

            let cmd = raw_network_message.cmd();

            // Try to write data, this may still fail with `WouldBlock` if
            // the readiness event is a false positive.
            match writable.as_ref().try_write(&msg) {
                Ok(n) => {
                    bandwidth
                        .total_bytes_outbound
                        .fetch_add(n as u64, Ordering::Relaxed);

                    // Bitcoin Core logs the message size without counting in the header.
                    let msg_len = msg.len().saturating_sub(MSG_HEADER_SIZE);
                    tracing::trace!(to = ?peer, "=> {cmd} ({msg_len} bytes) sent successfully");
                }
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::WouldBlock {
                        msg_buffer.push_back((cmd, msg));
                    } else {
                        return Err(e.into());
                    }
                }
            }
        }
    }
}
