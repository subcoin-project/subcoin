//! End-to-end tests for UTXO sync between two nodes.
//!
//! These tests simulate the P2P UTXO fast sync protocol:
//! - Server node: Has UTXOs and serves chunks to peers
//! - Client node: Downloads UTXO chunks and verifies with MuHash
//!
//! The tests use a mock protocol layer to test the storage layer integration
//! without requiring full network infrastructure.

use bitcoin::hashes::Hash;
use bitcoin::{OutPoint, ScriptBuf};
use std::sync::Arc;
use subcoin_bitcoin_state::{BitcoinState, Coin};

/// Mock UTXO sync protocol message types.
#[derive(Debug, Clone)]
pub enum UtxoSyncRequest {
    /// Request UTXO set info at a specific height.
    GetUtxoSetInfo,
    /// Request a chunk of UTXOs starting from cursor.
    GetUtxoChunk {
        cursor: Option<[u8; 36]>,
        max_entries: u32,
    },
}

#[derive(Debug, Clone)]
pub enum UtxoSyncResponse {
    /// UTXO set info response.
    UtxoSetInfo {
        height: u32,
        utxo_count: u64,
        muhash: String,
    },
    /// UTXO chunk response.
    UtxoChunk {
        entries: Vec<(OutPoint, Coin)>,
        next_cursor: Option<[u8; 36]>,
        is_complete: bool,
    },
    /// Error response.
    Error(String),
}

/// Mock server that serves UTXO chunks from its BitcoinState.
pub struct MockUtxoServer {
    state: Arc<BitcoinState>,
}

impl MockUtxoServer {
    pub fn new(state: Arc<BitcoinState>) -> Self {
        Self { state }
    }

    /// Handle incoming request and return response.
    pub fn handle_request(&self, request: UtxoSyncRequest) -> UtxoSyncResponse {
        match request {
            UtxoSyncRequest::GetUtxoSetInfo => UtxoSyncResponse::UtxoSetInfo {
                height: self.state.height(),
                utxo_count: self.state.utxo_count(),
                muhash: self.state.muhash_hex(),
            },
            UtxoSyncRequest::GetUtxoChunk {
                cursor,
                max_entries,
            } => match self.state.export_chunk(cursor.as_ref(), max_entries) {
                Ok((entries, next_cursor, is_complete)) => UtxoSyncResponse::UtxoChunk {
                    entries,
                    next_cursor,
                    is_complete,
                },
                Err(e) => UtxoSyncResponse::Error(format!("{e:?}")),
            },
        }
    }
}

/// Mock client that syncs UTXOs from a server.
pub struct MockUtxoClient {
    state: Arc<BitcoinState>,
    target_height: u32,
    expected_muhash: String,
    current_cursor: Option<[u8; 36]>,
    downloaded_utxos: u64,
    chunk_size: u32,
}

impl MockUtxoClient {
    pub fn new(
        state: Arc<BitcoinState>,
        target_height: u32,
        expected_muhash: String,
        chunk_size: u32,
    ) -> Self {
        Self {
            state,
            target_height,
            expected_muhash,
            current_cursor: None,
            downloaded_utxos: 0,
            chunk_size,
        }
    }

    /// Generate the next request to send to server.
    pub fn next_request(&self) -> Option<UtxoSyncRequest> {
        if self.is_complete() {
            return None;
        }

        Some(UtxoSyncRequest::GetUtxoChunk {
            cursor: self.current_cursor,
            max_entries: self.chunk_size,
        })
    }

    /// Process a response from the server.
    pub fn process_response(&mut self, response: UtxoSyncResponse) -> Result<(), String> {
        match response {
            UtxoSyncResponse::UtxoChunk {
                entries,
                next_cursor,
                is_complete,
            } => {
                let count = entries.len();

                // Import the chunk
                self.state
                    .bulk_import(entries)
                    .map_err(|e| format!("Bulk import failed: {e:?}"))?;

                self.downloaded_utxos += count as u64;
                self.current_cursor = next_cursor;

                if is_complete {
                    // Finalize and verify
                    self.state
                        .finalize_import(self.target_height)
                        .map_err(|e| format!("Finalize failed: {e:?}"))?;

                    // Verify MuHash
                    if !self.state.verify_muhash(&self.expected_muhash) {
                        return Err(format!(
                            "MuHash verification failed: expected {}, got {}",
                            self.expected_muhash,
                            self.state.muhash_hex()
                        ));
                    }
                }

                Ok(())
            }
            UtxoSyncResponse::UtxoSetInfo { .. } => {
                // Info response, just acknowledge
                Ok(())
            }
            UtxoSyncResponse::Error(e) => Err(e),
        }
    }

    /// Check if sync is complete and verified.
    pub fn is_complete(&self) -> bool {
        self.state.height() == self.target_height && self.state.verify_muhash(&self.expected_muhash)
    }

    /// Get progress info.
    pub fn progress(&self) -> (u64, bool) {
        (self.downloaded_utxos, self.is_complete())
    }
}

/// Helper to create test UTXOs with deterministic data.
fn create_test_utxos(count: usize, start_height: u32) -> Vec<(OutPoint, Coin)> {
    (0..count)
        .map(|i| {
            let mut txid_bytes = [0u8; 32];
            txid_bytes[0..8].copy_from_slice(&(i as u64).to_le_bytes());
            let txid = bitcoin::Txid::from_byte_array(txid_bytes);
            let outpoint = OutPoint { txid, vout: 0 };
            let coin = Coin::new(
                i == 0,                          // First one is coinbase
                (i as u64 + 1) * 50_000_000,     // 0.5 BTC increments
                start_height + (i as u32 / 100), // Spread across heights
                ScriptBuf::new_p2pkh(&bitcoin::PubkeyHash::all_zeros()).to_bytes(),
            );
            (outpoint, coin)
        })
        .collect()
}

/// Open a temporary BitcoinState for testing.
fn open_temp_state() -> Arc<BitcoinState> {
    let temp_dir = tempfile::tempdir().unwrap();
    Arc::new(BitcoinState::open(temp_dir.path()).unwrap())
}

// ============================================================================
// E2E Tests
// ============================================================================

#[test]
fn test_basic_two_node_sync() {
    // Server node with some UTXOs
    let server_state = open_temp_state();
    let utxos = create_test_utxos(100, 0);
    server_state.bulk_import(utxos).unwrap();
    server_state.finalize_import(100).unwrap();

    let server = MockUtxoServer::new(server_state.clone());

    // Get server info
    let info_response = server.handle_request(UtxoSyncRequest::GetUtxoSetInfo);
    let (target_height, expected_muhash) = match info_response {
        UtxoSyncResponse::UtxoSetInfo { height, muhash, .. } => (height, muhash),
        _ => panic!("Expected UtxoSetInfo response"),
    };

    // Client node starts empty
    let client_state = open_temp_state();
    let mut client = MockUtxoClient::new(client_state.clone(), target_height, expected_muhash, 30);

    // Sync loop
    let mut iterations = 0;
    while let Some(request) = client.next_request() {
        let response = server.handle_request(request);
        client.process_response(response).unwrap();
        iterations += 1;

        if iterations > 10 {
            panic!("Sync took too many iterations");
        }
    }

    assert!(client.is_complete());
    assert_eq!(client_state.height(), server_state.height());
    assert_eq!(client_state.utxo_count(), server_state.utxo_count());
    assert_eq!(client_state.muhash_hex(), server_state.muhash_hex());
}

#[test]
fn test_large_utxo_set_sync() {
    // Server with many UTXOs
    let server_state = open_temp_state();
    let utxos = create_test_utxos(1000, 0);
    server_state.bulk_import(utxos).unwrap();
    server_state.finalize_import(500).unwrap();

    let server = MockUtxoServer::new(server_state.clone());

    // Get server info
    let info_response = server.handle_request(UtxoSyncRequest::GetUtxoSetInfo);
    let (target_height, expected_muhash, expected_count) = match info_response {
        UtxoSyncResponse::UtxoSetInfo {
            height,
            muhash,
            utxo_count,
        } => (height, muhash, utxo_count),
        _ => panic!("Expected UtxoSetInfo response"),
    };

    assert_eq!(expected_count, 1000);

    // Client syncs with smaller chunks
    let client_state = open_temp_state();
    let mut client = MockUtxoClient::new(client_state.clone(), target_height, expected_muhash, 100);

    let mut chunk_count = 0;
    while let Some(request) = client.next_request() {
        let response = server.handle_request(request);
        client.process_response(response).unwrap();
        chunk_count += 1;
    }

    assert!(client.is_complete());
    assert_eq!(chunk_count, 10); // 1000 UTXOs / 100 per chunk
    assert_eq!(client_state.utxo_count(), 1000);
}

#[test]
fn test_sync_with_muhash_mismatch() {
    // Server with UTXOs
    let server_state = open_temp_state();
    let utxos = create_test_utxos(50, 0);
    server_state.bulk_import(utxos).unwrap();
    server_state.finalize_import(50).unwrap();

    let server = MockUtxoServer::new(server_state.clone());

    // Client with WRONG expected muhash
    let client_state = open_temp_state();
    let wrong_muhash = "0".repeat(64);
    let mut client = MockUtxoClient::new(client_state.clone(), 50, wrong_muhash, 100);

    // Sync all chunks
    while let Some(request) = client.next_request() {
        let response = server.handle_request(request);
        let result = client.process_response(response);

        // The last chunk should fail verification
        if result.is_err() {
            assert!(result.unwrap_err().contains("MuHash verification failed"));
            return;
        }
    }

    panic!("Expected MuHash verification to fail");
}

#[test]
fn test_incremental_sync_progress() {
    let server_state = open_temp_state();
    let utxos = create_test_utxos(250, 0);
    server_state.bulk_import(utxos).unwrap();
    server_state.finalize_import(100).unwrap();

    let server = MockUtxoServer::new(server_state.clone());

    let info_response = server.handle_request(UtxoSyncRequest::GetUtxoSetInfo);
    let (target_height, expected_muhash) = match info_response {
        UtxoSyncResponse::UtxoSetInfo { height, muhash, .. } => (height, muhash),
        _ => panic!("Expected UtxoSetInfo response"),
    };

    let client_state = open_temp_state();
    let mut client = MockUtxoClient::new(client_state.clone(), target_height, expected_muhash, 50);

    let mut progress_checks = Vec::new();

    while let Some(request) = client.next_request() {
        let response = server.handle_request(request);
        client.process_response(response).unwrap();

        let (downloaded, complete) = client.progress();
        progress_checks.push((downloaded, complete));
    }

    // Verify incremental progress
    assert_eq!(progress_checks.len(), 5); // 250 / 50 = 5 chunks
    assert_eq!(progress_checks[0], (50, false));
    assert_eq!(progress_checks[1], (100, false));
    assert_eq!(progress_checks[2], (150, false));
    assert_eq!(progress_checks[3], (200, false));
    assert_eq!(progress_checks[4], (250, true)); // Last chunk completes sync
}

#[test]
fn test_empty_utxo_set_sync() {
    // Server with no UTXOs (but finalized at height)
    let server_state = open_temp_state();
    server_state.finalize_import(0).unwrap();

    let server = MockUtxoServer::new(server_state.clone());

    let info_response = server.handle_request(UtxoSyncRequest::GetUtxoSetInfo);
    let (target_height, expected_muhash, expected_count) = match info_response {
        UtxoSyncResponse::UtxoSetInfo {
            height,
            muhash,
            utxo_count,
        } => (height, muhash, utxo_count),
        _ => panic!("Expected UtxoSetInfo response"),
    };

    assert_eq!(expected_count, 0);

    let client_state = open_temp_state();
    let mut client = MockUtxoClient::new(client_state.clone(), target_height, expected_muhash, 100);

    // Single request should complete
    if let Some(request) = client.next_request() {
        let response = server.handle_request(request);
        client.process_response(response).unwrap();
    }

    assert!(client.is_complete());
    assert_eq!(client_state.utxo_count(), 0);
}

#[test]
fn test_sync_restart_after_failure() {
    // Server with UTXOs
    let server_state = open_temp_state();
    let utxos = create_test_utxos(100, 0);
    server_state.bulk_import(utxos).unwrap();
    server_state.finalize_import(50).unwrap();

    let server = MockUtxoServer::new(server_state.clone());

    let info_response = server.handle_request(UtxoSyncRequest::GetUtxoSetInfo);
    let (target_height, expected_muhash) = match info_response {
        UtxoSyncResponse::UtxoSetInfo { height, muhash, .. } => (height, muhash),
        _ => panic!("Expected UtxoSetInfo response"),
    };

    // Client starts sync but gets interrupted
    let client_state = open_temp_state();
    let mut client = MockUtxoClient::new(
        client_state.clone(),
        target_height,
        expected_muhash.clone(),
        30,
    );

    // Download only first chunk
    if let Some(request) = client.next_request() {
        let response = server.handle_request(request);
        client.process_response(response).unwrap();
    }

    assert!(!client.is_complete());
    assert_eq!(client_state.utxo_count(), 30);

    // Simulate restart: clear and start fresh
    client_state.clear().unwrap();
    assert_eq!(client_state.utxo_count(), 0);

    // Create new client and sync from scratch
    let mut client = MockUtxoClient::new(client_state.clone(), target_height, expected_muhash, 50);

    while let Some(request) = client.next_request() {
        let response = server.handle_request(request);
        client.process_response(response).unwrap();
    }

    assert!(client.is_complete());
    assert_eq!(client_state.utxo_count(), 100);
}

#[test]
fn test_concurrent_export_import() {
    // Test that export and import can happen on different threads
    use std::thread;

    let server_state = open_temp_state();
    let utxos = create_test_utxos(500, 0);
    server_state.bulk_import(utxos).unwrap();
    server_state.finalize_import(100).unwrap();

    let target_height = server_state.height();
    let expected_muhash = server_state.muhash_hex();

    // Export in one thread
    let export_state = server_state.clone();
    let export_handle = thread::spawn(move || {
        let mut all_utxos = Vec::new();
        let mut cursor = None;

        loop {
            let (chunk, next_cursor, is_complete) =
                export_state.export_chunk(cursor.as_ref(), 100).unwrap();
            all_utxos.extend(chunk);
            if is_complete {
                break;
            }
            cursor = next_cursor;
        }

        all_utxos
    });

    let exported_utxos = export_handle.join().unwrap();
    assert_eq!(exported_utxos.len(), 500);

    // Import in another thread
    let client_state = open_temp_state();
    let import_state = client_state.clone();
    let import_handle = thread::spawn(move || {
        import_state.bulk_import(exported_utxos).unwrap();
        import_state.finalize_import(target_height).unwrap();
    });

    import_handle.join().unwrap();

    assert_eq!(client_state.height(), target_height);
    assert_eq!(client_state.muhash_hex(), expected_muhash);
}

#[test]
fn test_utxo_ordering_consistency() {
    // Verify that UTXOs are always exported in the same order
    let state = open_temp_state();
    let utxos = create_test_utxos(100, 0);
    state.bulk_import(utxos).unwrap();

    // Export twice and compare
    let (export1, _, _) = state.export_chunk(None, 100).unwrap();
    let (export2, _, _) = state.export_chunk(None, 100).unwrap();

    assert_eq!(export1.len(), export2.len());
    for (a, b) in export1.iter().zip(export2.iter()) {
        assert_eq!(a.0, b.0, "OutPoints should match");
        assert_eq!(a.1, b.1, "Coins should match");
    }
}

#[test]
fn test_chunk_boundary_handling() {
    // Test exact chunk boundaries
    let server_state = open_temp_state();

    // Import exactly 100 UTXOs
    let utxos = create_test_utxos(100, 0);
    server_state.bulk_import(utxos).unwrap();
    server_state.finalize_import(50).unwrap();

    // Request chunk of exactly 100
    let (chunk, next_cursor, is_complete) = server_state.export_chunk(None, 100).unwrap();

    assert_eq!(chunk.len(), 100);
    assert!(is_complete);
    assert!(next_cursor.is_none());

    // Request chunk of 101 (more than available)
    let (chunk, next_cursor, is_complete) = server_state.export_chunk(None, 101).unwrap();

    assert_eq!(chunk.len(), 100);
    assert!(is_complete);
    assert!(next_cursor.is_none());
}
