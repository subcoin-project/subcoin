use crate::{Local, PeerId};
use bitcoin::consensus::{deserialize_partial, Encodable};
use bitcoin::p2p::message::{NetworkMessage, RawNetworkMessage, MAX_MSG_SIZE};
use bitcoin::p2p::message_blockdata::Inventory;
use bitcoin::p2p::message_network::VersionMessage;
use bitcoin::p2p::{Address, ServiceFlags};
use bitcoin::{Block, BlockHash};
use parking_lot::RwLock;
use sc_client_api::HeaderBackend;
use sc_service::SpawnTaskHandle;
use std::collections::HashMap;
use std::sync::Arc;
use subcoin_service::{new_node, NodeComponents, SubcoinConfiguration};
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;
use tokio::runtime::Handle;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

#[derive(Clone)]
struct MockBitcoind {
    hash2number: HashMap<BlockHash, u32>,
    blocks: Vec<Block>,
    local_addr: PeerId,
    connections: Arc<RwLock<HashMap<PeerId, UnboundedSender<NetworkMessage>>>>,
}

impl MockBitcoind {
    fn new(local_addr: PeerId) -> Self {
        let blocks = subcoin_test_service::block_data();
        let mut hash2number = HashMap::new();
        blocks.iter().enumerate().for_each(|(index, block)| {
            let block_hash = block.block_hash();
            hash2number.insert(block_hash, index as u32);
        });
        Self {
            hash2number,
            blocks,
            local_addr,
            connections: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    // MockBitcoind only supports the inbound connection.
    async fn handle_message(&mut self, msg: NetworkMessage, from: PeerId) -> Result<(), ()> {
        match msg {
            NetworkMessage::Version(_v) => {
                let services = ServiceFlags::NETWORK | ServiceFlags::WITNESS;

                // Send our version.
                let nonce = 666;
                let local_time = Local::now();
                let our_version = VersionMessage {
                    version: 70016,
                    services,
                    timestamp: local_time.timestamp(),
                    receiver: Address::new(&from, ServiceFlags::NONE),
                    sender: Address::new(&self.local_addr, services),
                    nonce,
                    user_agent: "/MockBitcoind:0.0.1".to_string(),
                    start_height: 100,
                    relay: false,
                };
                self.send(from, NetworkMessage::Version(our_version));
                self.send(from, NetworkMessage::Verack);
            }
            NetworkMessage::Verack => {}
            NetworkMessage::GetAddr => {
                self.send(from, NetworkMessage::AddrV2(Vec::new()));
            }
            NetworkMessage::GetData(invs) => {
                for inv in invs {
                    match inv {
                        Inventory::Block(block_hash) => {
                            let block = self
                                .hash2number
                                .get(&block_hash)
                                .and_then(|number| self.blocks.get(*number as usize))
                                .expect("Block not found");
                            self.send(from, NetworkMessage::Block(block.clone()));
                        }
                        unsupported_inv => todo!("Handle {unsupported_inv:?}"),
                    }
                }
            }
            NetworkMessage::Ping(nonce) => {
                self.send(from, NetworkMessage::Pong(nonce));
            }
            msg => panic!("Unsupported NetworkMessage: {msg:?}"),
        }

        Ok(())
    }

    fn send(&self, peer: PeerId, msg: NetworkMessage) {
        self.connections
            .read()
            .get(&peer)
            .expect("Peer not found")
            .send(msg)
            .expect("Failed to send peer message");
    }
}

async fn bitcoind_main_loop(
    bitcoind: MockBitcoind,
    listener: TcpListener,
    network: bitcoin::Network,
    spawn_handle: SpawnTaskHandle,
) {
    loop {
        let (socket, peer_addr) = listener
            .accept()
            .await
            .unwrap_or_else(|err| panic!("Failed to accept inbound connection: {err:?}"));

        tracing::debug!("Accepted inbound connection from {peer_addr:?}");

        let (mut reader, writer) = socket.into_split();

        let reader_fut = {
            let mock_bitcoind = bitcoind.clone();

            async move {
                let mut unparsed = vec![];
                loop {
                    let mut buf = vec![0; 1024];
                    let bytes_read = reader.read(&mut buf).await.unwrap();
                    buf.truncate(bytes_read);
                    unparsed.extend(buf.iter());

                    while !unparsed.is_empty() {
                        let mut mock_bitcoind = mock_bitcoind.clone();

                        match deserialize_partial::<RawNetworkMessage>(&unparsed) {
                            Ok((raw, consumed)) => {
                                if let Err(err) = mock_bitcoind.handle_message(raw.into_payload(), peer_addr).await {
                                    eprintln!("Mock bitcoind handler error: {err:?}");
                                }

                                unparsed.drain(..consumed);
                            }
                            Err(bitcoin::consensus::encode::Error::Io(ref err)) // Received incomplete message
                                if err.kind() == bitcoin::io::ErrorKind::UnexpectedEof =>
                            {
                                break
                            }
                            Err(err) => panic!("Error occurred in parsing network message: {err}"),
                        }
                    }
                }
            }
        };

        spawn_handle.spawn("bitcoind-reader", None, reader_fut);

        let (sender, mut receiver) = unbounded_channel();

        let mut connections = bitcoind.connections.write();
        connections.insert(peer_addr, sender);
        drop(connections);

        let writer_fut = async move {
            let magic = network.magic();

            while let Some(network_message) = receiver.recv().await {
                writer
                    .writable()
                    .await
                    .expect("Failed to await connection writer");

                let raw_network_message = RawNetworkMessage::new(magic, network_message);

                let mut msg = Vec::with_capacity(MAX_MSG_SIZE);
                raw_network_message
                    .consensus_encode(&mut msg)
                    .expect("Failed to encode raw network message");

                let cmd = raw_network_message.cmd();

                tracing::trace!("Sending {cmd}");

                writer
                    .try_write(&msg)
                    .expect("Failed to send message to peer");
            }
        };

        spawn_handle.spawn("bitcoind-writer", None, writer_fut);
    }
}

async fn new_mock_bitcoind(spawn_handle: SpawnTaskHandle) -> MockBitcoind {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();

    let addr = listener.local_addr().unwrap();

    tracing::debug!("MockBitcoind listens on {addr:?}");

    let bitcoind = MockBitcoind::new(addr);

    spawn_handle.spawn("bitcoind-node", None, {
        bitcoind_main_loop(
            bitcoind.clone(),
            listener,
            bitcoin::Network::Bitcoin,
            spawn_handle.clone(),
        )
    });

    bitcoind
}

#[tokio::test]
async fn test_block_announcement_via_headers() {
    sp_tracing::try_init_simple();

    let runtime_handle = Handle::current();

    let config = subcoin_test_service::test_configuration(runtime_handle);

    let base_path = config.base_path.path().to_path_buf();

    let NodeComponents {
        client,
        task_manager,
        ..
    } = new_node(SubcoinConfiguration {
        network: bitcoin::Network::Bitcoin,
        config: &config,
        no_hardware_benchmarks: true,
        storage_monitor: Default::default(),
    })
    .expect("Failed to create node");

    let spawn_handle = task_manager.spawn_handle();

    let bitcoind = new_mock_bitcoind(spawn_handle.clone()).await;

    let bitcoin_block_import = sc_consensus_nakamoto::BitcoinBlockImporter::<
        _,
        _,
        _,
        _,
        subcoin_service::TransactionAdapter,
    >::new(
        client.clone(),
        client.clone(),
        sc_consensus_nakamoto::ImportConfig {
            network: bitcoin::Network::Bitcoin,
            block_verification: sc_consensus_nakamoto::BlockVerification::Full,
            execute_block: true,
            verify_script: true,
        },
        Arc::new(subcoin_service::CoinStorageKey),
        None,
    );

    let import_queue = sc_consensus_nakamoto::bitcoin_import_queue(
        &task_manager.spawn_essential_handle(),
        bitcoin_block_import,
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();

    let listen_on = listener.local_addr().unwrap();

    tracing::debug!("Subcoin listens on {listen_on:?}");

    let (subcoin_networking, _subcoin_network_handle) = crate::Network::new(
        client.clone(),
        crate::Config {
            network: bitcoin::Network::Bitcoin,
            listen_on,
            seednodes: vec![bitcoind.local_addr.to_string()],
            seednode_only: true,
            ipv4_only: true,
            max_outbound_peers: 10,
            max_inbound_peers: 10,
            persistent_peer_latency_threshold: 200,
            sync_strategy: crate::SyncStrategy::HeadersFirst,
            enable_block_sync_on_startup: false,
            base_path,
        },
        import_queue,
        spawn_handle.clone(),
        None,
    );

    task_manager
        .spawn_essential_handle()
        .spawn("subcoin-networking", None, async move {
            if let Err(err) = subcoin_networking.run().await {
                panic!("Fatal error in subcoin networking: {err:?}");
            }
        });

    // Wait for the connection to be established.
    for _ in 0..10 {
        if bitcoind.connections.read().keys().next().is_none() {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        } else {
            break;
        }
    }

    let subcoin_node = bitcoind
        .connections
        .read()
        .keys()
        .next()
        .copied()
        .expect("Connection has been established");

    // Assume block 1 is a new block and broadcast it to the subcoin node via headers.
    let header1 = bitcoind.blocks[1].header.clone();
    bitcoind.send(subcoin_node, NetworkMessage::Headers(vec![header1]));

    // TODO: could be flaky.
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    assert_eq!(client.info().best_number, 1);
}
