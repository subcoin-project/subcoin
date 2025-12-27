# P2P UTXO Fast Sync Implementation

## Overview

Custom P2P UTXO chunk download protocol that populates `BitcoinState` (native RocksDB storage) with MuHash verification against Bitcoin Core checkpoints.

**Features:**
- P2P UTXO chunk protocol (fully decentralized)
- MuHash verification against Bitcoin Core checkpoints
- Fully decoupled from Substrate state sync
- Cursor-based pagination for resumable downloads

## Implementation Status

| Phase | Description | Status |
|-------|-------------|--------|
| 1 | Storage Extensions | ✅ Complete |
| 2 | Protocol Messages | ✅ Complete |
| 3 | UTXO Sync Handler | ✅ Complete |
| 4 | UTXO Sync Strategy | ✅ Complete |
| 5 | Integration | ✅ Complete |
| 6 | Wire BitcoinState | ✅ Complete |

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                      Snapcake Node (Client)                      │
├─────────────────────────────────────────────────────────────────┤
│  SnapcakeSyncingStrategy                                        │
│    ├── UtxoSnapSync (downloads UTXOs from peers)                │
│    │     ├── Sends GetUtxoChunk requests                        │
│    │     ├── Accumulates UTXOs with cursor pagination           │
│    │     └── Verifies final MuHash against checkpoint           │
│    └── ChainSync (resumes after UTXO sync complete)             │
├─────────────────────────────────────────────────────────────────┤
│  BitcoinState (RocksDB)                                         │
│    ├── bulk_import() - Import UTXO chunks                       │
│    ├── finalize_import() - Set height and verify MuHash         │
│    └── Stores UTXOs with MuHash commitment                      │
└─────────────────────────────────────────────────────────────────┘
                              │
                              │ P2P Protocol
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                      Full Node (Server)                          │
├─────────────────────────────────────────────────────────────────┤
│  NetworkRequestHandler                                          │
│    ├── Handles GetUtxoSetInfo requests                          │
│    ├── Handles GetUtxoChunk requests                            │
│    └── Returns UtxoChunk responses with cursor                  │
├─────────────────────────────────────────────────────────────────┤
│  BitcoinState (RocksDB)                                         │
│    ├── export_chunk() - Serve UTXO chunks to peers              │
│    └── Full UTXO set at synced height                           │
└─────────────────────────────────────────────────────────────────┘
```

---

## Key Components

### 1. BitcoinState (`crates/subcoin-bitcoin-state/src/storage.rs`)

Native RocksDB storage for UTXOs with MuHash commitment.

```rust
impl BitcoinState {
    /// Bulk import UTXOs for fast sync.
    pub fn bulk_import(&self, entries: Vec<UtxoEntry>) -> Result<()>

    /// Export a chunk of UTXOs for serving to peers.
    pub fn export_chunk(
        &self,
        cursor: Option<&[u8]>,
        max_entries: usize,
    ) -> Result<(Vec<UtxoEntry>, Option<Vec<u8>>, bool)>

    /// Finalize bulk import at a specific height.
    pub fn finalize_import(&self, height: u32) -> Result<()>

    /// Verify and repair MuHash if inconsistent.
    pub fn verify_and_repair_muhash(&self) -> Result<(bool, u64, String)>

    /// Get the 32-byte txoutset MuHash (compatible with Bitcoin Core).
    pub fn muhash_hex(&self) -> String
}
```

### 2. Checkpoints (`crates/subcoin-bitcoin-state/src/checkpoints.json`)

MuHash checkpoints verified against Bitcoin Core v29.99.0:

| Height | UTXO Count | MuHash |
|--------|------------|--------|
| 0 | 0 | `dd5ad2a1...2555c8` |
| 100,000 | 71,888 | `86aa993b...831e3e` |
| 200,000 | 2,318,056 | `169c05b5...3c8ced` |
| 300,000 | 10,852,334 | `fc15451c...2683a1` |
| 400,000 | 34,820,275 | `95bce37e...97d3e2` |
| 500,000 | 59,949,466 | `cf4a51b5...9104c1` |
| 600,000 | 63,389,760 | `daed1b24...3238c9` |
| 700,000 | 75,262,612 | `35cab51d...71c897` |
| 750,000 | 83,702,786 | `ee94f063...7c5371` |
| 800,000 | 111,535,121 | `3e22f8ce...62d730` |
| 840,000 | 176,948,713 | `ba56574e...65a637` |

Checkpoints loaded at compile time from JSON with metadata:
```json
{
  "metadata": {
    "bitcoin_core_version": "v29.99.0-2def85847318",
    "generated_at": "2025-12-24",
    "command": "bitcoin-cli gettxoutsetinfo muhash <height>"
  },
  "checkpoints": [...]
}
```

### 3. Protocol Messages (`crates/subcoin-service/src/network_request_handler.rs`)

```rust
pub enum NetworkRequest<Block> {
    // ... existing requests ...
    GetUtxoSetInfo,
    GetUtxoChunk {
        cursor: Option<Vec<u8>>,
        max_entries: u32,
    },
}

pub enum NetworkResponse<Block> {
    // ... existing responses ...
    UtxoSetInfo {
        height: u32,
        utxo_count: u64,
        muhash: MuHashCommitment,
    },
    UtxoChunk {
        entries: Vec<UtxoEntry>,
        next_cursor: Option<Vec<u8>>,
        is_complete: bool,
    },
}
```

### 4. UTXO Snap Sync (`crates/subcoin-snapcake/src/utxo_sync.rs`)

```rust
/// UTXO Snapshot Sync - Downloads UTXO set from peers for fast node bootstrapping.
pub struct UtxoSnapSync<B: BlockT> {
    target_height: u32,
    expected_muhash: String,
    bitcoin_state: Arc<BitcoinState>,
    current_cursor: Option<[u8; 36]>,
    downloaded_utxos: u64,
    pending_requests: HashMap<PeerId, PendingRequest>,
    peer_states: HashMap<PeerId, PeerState>,
    connected_peers: HashSet<PeerId>,
}

impl UtxoSnapSync {
    pub fn new(target_height, expected_muhash, protocol_name, bitcoin_state) -> Self
    pub fn actions(&mut self, network: &NetworkServiceHandle) -> Vec<SyncingAction>
    pub fn on_utxo_chunk(&mut self, peer_id, entries, cursor, is_complete) -> Result<()>
    pub fn is_complete(&self) -> bool
    pub fn progress(&self) -> UtxoSnapSyncProgress
}
```

---

## Usage

### Running a Full Node (UTXO Server)

```bash
./target/release/subcoin run --chain bitcoin-mainnet
```

The full node serves UTXO chunks to peers via the P2P protocol.

### Running Snapcake (Fast Sync Client)

```bash
./target/release/snapcake \
    --chain bitcoin-mainnet \
    --fast-sync \
    --fast-sync-target-height 840000
```

Options:
- `--fast-sync`: Enable UTXO fast sync mode
- `--fast-sync-target-height <HEIGHT>`: Target checkpoint height (default: highest available)

---

## Key Design Decisions

1. **Lexicographic ordering**: UTXOs exported/imported in `(txid, vout)` order for deterministic pagination
2. **Cursor-based pagination**: Allows resumable downloads and parallel requests
3. **Final MuHash verification**: Only verify at completion (not per-chunk) for simplicity
4. **No undo data during fast sync**: Trust checkpoint; reorg below checkpoint requires full resync
5. **50,000 UTXOs per chunk**: ~5 MB per chunk, balances memory and network efficiency

---

## Bug Fixes

### BIP30 Duplicate Coinbase Handling

Bitcoin had duplicate coinbase txids in blocks 91722/91812 and 91880/91842. The storage layer now correctly handles this edge case by:
1. Detecting duplicate outpoints before insertion
2. Removing old coin from MuHash before adding new one
3. Not double-counting UTXOs

---

## Testing

### Unit Tests

```bash
cargo test -p subcoin-bitcoin-state
```

Tests include:
- `test_bulk_import_and_export` - Round-trip import/export
- `test_export_chunk_pagination` - Cursor-based pagination
- `test_muhash_consistency_after_bulk_import` - MuHash verification
- `test_duplicate_coinbase_txid_handling` - BIP30 edge case

### Integration Tests

```bash
cargo test -p subcoin-test-service
```

Tests include:
- `test_block_import_count_consistency` - Count/MuHash consistency after block import
- `test_utxo_sync_after_block_import` - Full UTXO sync flow from source to target

### End-to-End Tests

```bash
cargo test -p subcoin-bitcoin-state --test utxo_sync_e2e
```

Tests include:
- `test_utxo_sync_cursor_based` - Cursor-based sync flow
- `test_utxo_count_and_muhash_consistency` - Consistency verification

---

## Obtaining New Checkpoints

To add checkpoints for new heights, run against Bitcoin Core:

```bash
bitcoin-cli gettxoutsetinfo muhash <block_height>
```

Requirements:
- Bitcoin Core with `coinstatsindex=1` enabled
- Fully synced node

Then update `crates/subcoin-bitcoin-state/src/checkpoints.json`.

---

## Files Modified

| File | Description |
|------|-------------|
| `crates/subcoin-bitcoin-state/src/storage.rs` | BitcoinState with bulk_import, export_chunk, verify_and_repair_muhash |
| `crates/subcoin-bitcoin-state/src/checkpoints.rs` | Checkpoint loader from JSON |
| `crates/subcoin-bitcoin-state/src/checkpoints.json` | Bitcoin Core verified checkpoints |
| `crates/subcoin-service/src/network_request_handler.rs` | UTXO sync protocol messages |
| `crates/subcoin-snapcake/src/utxo_sync.rs` | UTXO sync strategy |
| `crates/subcoin-snapcake/src/syncing_strategy.rs` | Integration with SnapcakeSyncingStrategy |
| `crates/subcoin-snapcake/src/cli.rs` | CLI options for fast sync |
| `crates/subcoin-test-service/src/lib.rs` | Integration tests |

---

## Future Improvements

1. **Parallel chunk downloads**: Download from multiple peers simultaneously
2. **Incremental MuHash verification**: Verify per-chunk to detect corruption early
3. **Checkpoint auto-update**: Fetch new checkpoints from trusted sources
4. **Progress persistence**: Resume downloads after restart
