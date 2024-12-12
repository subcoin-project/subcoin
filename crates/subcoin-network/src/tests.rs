use crate::peer_connection::Direction;
use crate::sync::{SyncAction, SyncRequest};
use crate::{Local, NetworkHandle, PeerId, SyncStrategy};
use bitcoin::consensus::{deserialize_partial, Encodable};
use bitcoin::p2p::message::{NetworkMessage, RawNetworkMessage, MAX_MSG_SIZE};
use bitcoin::p2p::message_blockdata::Inventory;
use bitcoin::p2p::message_network::VersionMessage;
use bitcoin::p2p::{Address, ServiceFlags};
use bitcoin::{Block, BlockHash};
use parking_lot::RwLock;
use sc_client_api::HeaderBackend;
use sc_service::{SpawnTaskHandle, TaskManager};
use std::collections::HashMap;
use std::path::PathBuf;
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
        tracing::debug!("Processing {} from {from:?}", msg.cmd());
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

                tracing::trace!("Sending {cmd} to {peer_addr:?}");

                writer
                    .try_write(&msg)
                    .expect("Failed to send message to peer");
            }
        };

        spawn_handle.spawn("bitcoind-writer", None, writer_fut);
    }
}

#[sc_tracing::logging::prefix_logs_with("Bitcoind")]
async fn new_mock_bitcoind(spawn_handle: SpawnTaskHandle) -> MockBitcoind {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();

    let addr = listener.local_addr().unwrap();

    tracing::debug!("listens on {addr:?}");

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

struct TestNode {
    client: Arc<subcoin_service::FullClient>,
    backend: Arc<subcoin_service::FullBackend>,
    base_path: PathBuf,
    task_manager: TaskManager,
}

impl TestNode {
    async fn new(runtime_handle: Handle) -> Self {
        let config = subcoin_test_service::test_configuration(runtime_handle);

        let base_path = config.base_path.path().to_path_buf();

        let NodeComponents {
            client,
            backend,
            task_manager,
            ..
        } = new_node(SubcoinConfiguration {
            network: bitcoin::Network::Bitcoin,
            config: &config,
            no_hardware_benchmarks: true,
            storage_monitor: Default::default(),
        })
        .expect("Failed to create node");

        Self {
            client,
            backend,
            base_path,
            task_manager,
        }
    }

    #[sc_tracing::logging::prefix_logs_with("Subcoin")]
    async fn start_network(
        &self,
        seednodes: Vec<String>,
        sync_strategy: SyncStrategy,
    ) -> NetworkHandle {
        let bitcoin_block_import = sc_consensus_nakamoto::BitcoinBlockImporter::<
            _,
            _,
            _,
            _,
            subcoin_service::TransactionAdapter,
        >::new(
            self.client.clone(),
            self.client.clone(),
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
            &self.task_manager.spawn_essential_handle(),
            bitcoin_block_import,
        );

        crate::build_network(
            self.client.clone(),
            crate::Config {
                network: bitcoin::Network::Bitcoin,
                listen_on: "127.0.0.1:0".parse().unwrap(),
                seednodes,
                seednode_only: true,
                ipv4_only: true,
                sync_target: None,
                max_outbound_peers: 10,
                max_inbound_peers: 10,
                min_sync_peer_threshold: 0,
                persistent_peer_latency_threshold: 200,
                sync_strategy,
                block_sync: crate::BlockSyncOption::Off,
                base_path: self.base_path.clone(),
            },
            import_queue,
            &self.task_manager,
            None,
            None,
        )
        .await
        .unwrap()
    }
}

#[tokio::test]
async fn block_announcement_via_headers_should_work() {
    let _ = sc_tracing::logging::LoggerBuilder::new("").init();

    let runtime_handle = Handle::current();

    let test_node = TestNode::new(runtime_handle).await;
    let bitcoind = new_mock_bitcoind(test_node.task_manager.spawn_handle()).await;

    let network_handle = test_node
        .start_network(
            vec![bitcoind.local_addr.to_string()],
            SyncStrategy::HeadersFirst,
        )
        .await;

    // Wait for the connection to be established.
    let _subcoin_node_addr = loop {
        if let Some(addr) = network_handle.local_addr_for(bitcoind.local_addr).await {
            break addr;
        } else {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    };

    let block1 = bitcoind.blocks[1].clone();
    let header1 = block1.header.clone();

    // Receive new block #1 in headers.
    let sync_action = network_handle
        .process_network_message(
            bitcoind.local_addr,
            Direction::Outbound,
            NetworkMessage::Headers(vec![header1.clone()]),
        )
        .await
        .unwrap();

    if let SyncAction::Request(SyncRequest::Data(inv, from)) = &sync_action {
        assert_eq!(inv, &[Inventory::Block(header1.block_hash())]);
        assert_eq!(*from, bitcoind.local_addr);
    } else {
        panic!("Expected SyncAction::Request(SyncRequest::Data), got: {sync_action:?}");
    }

    // Download the block data for #1.
    network_handle.execute_sync_action(sync_action).await;

    network_handle
        .process_network_message(
            bitcoind.local_addr,
            Direction::Outbound,
            NetworkMessage::Block(block1),
        )
        .await
        .unwrap();

    // Wait for some time as the block will be imported in async.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    assert_eq!(test_node.client.info().best_number, 1);
}
