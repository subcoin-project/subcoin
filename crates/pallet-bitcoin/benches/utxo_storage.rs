//! Benchmark comparing StorageDoubleMap vs StorageMap<BoundedVec> for UTXO storage.
//!
//! This benchmark measures the performance impact of reducing trie fan-out by storing
//! all transaction outputs together instead of each UTXO as a separate trie leaf.
//!
//! Run with: cargo bench --package pallet-bitcoin --bench utxo_storage

use codec::{Decode, Encode, MaxEncodedLen};
use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use frame_support::traits::ConstU32;
use frame_support::{BoundedVec, construct_runtime, derive_impl};
use sp_io::TestExternalities;
use sp_runtime::BuildStorage;
use sp_runtime::traits::IdentityLookup;

type Txid = [u8; 32];
type Vout = u32;

/// Maximum outputs per transaction (Bitcoin consensus limit is ~200k, but practically much less)
type MaxOutputs = ConstU32<1000>;

#[derive(Clone, Encode, Decode, Debug, PartialEq, Eq, MaxEncodedLen, scale_info::TypeInfo)]
struct Coin {
    value: u64,
    script_pubkey: BoundedVec<u8, ConstU32<10000>>,
    height: u32,
}

impl Default for Coin {
    fn default() -> Self {
        Self::dummy(0)
    }
}

impl Coin {
    fn dummy(value: u64) -> Self {
        Self {
            value,
            // P2PKH script (25 bytes)
            script_pubkey: vec![
                0x76, 0xa9, 0x14, // OP_DUP OP_HASH160 PUSH(20)
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, // 20-byte pubkey hash
                0x88, 0xac, // OP_EQUALVERIFY OP_CHECKSIG
            ]
            .try_into()
            .unwrap(),
            height: 800000,
        }
    }
}

#[derive(Clone, Encode, Decode, Debug, PartialEq, Eq, MaxEncodedLen, scale_info::TypeInfo)]
struct Output {
    vout: u32,
    coin: Coin,
}

impl Default for Output {
    fn default() -> Self {
        Self {
            vout: 0,
            coin: Coin::default(),
        }
    }
}

// Mock runtime for testing
#[frame_support::pallet]
mod pallet_double_map {
    use super::*;
    use frame_support::pallet_prelude::*;

    #[pallet::config]
    pub trait Config: frame_system::Config {}

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    /// Storage: One trie leaf per UTXO (high fan-out)
    #[pallet::storage]
    pub type Coins<T: Config> =
        StorageDoubleMap<_, Blake2_128Concat, Txid, Blake2_128Concat, Vout, Coin, ValueQuery>;
}

#[frame_support::pallet]
mod pallet_bounded_vec {
    use super::*;
    use frame_support::pallet_prelude::*;

    #[pallet::config]
    pub trait Config: frame_system::Config {}

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    /// Storage: One trie leaf per transaction (low fan-out)
    #[pallet::storage]
    pub type Outputs<T: Config> =
        StorageMap<_, Blake2_128Concat, Txid, BoundedVec<Output, MaxOutputs>, ValueQuery>;
}

type Block = frame_system::mocking::MockBlock<Test>;

construct_runtime!(
    pub enum Test {
        System: frame_system,
        DoubleMap: pallet_double_map,
        OutputVec: pallet_bounded_vec,
    }
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = Block;
    type AccountId = u64;
    type Lookup = IdentityLookup<Self::AccountId>;
}

impl pallet_double_map::Config for Test {}
impl pallet_bounded_vec::Config for Test {}

struct BenchTx {
    txid: Txid,
    outputs: Vec<Output>,
}

impl BenchTx {
    fn new(id: u32, output_count: usize) -> Self {
        let mut txid = [0u8; 32];
        txid[0..4].copy_from_slice(&id.to_le_bytes());

        let outputs = (0..output_count)
            .map(|i| Output {
                vout: i as u32,
                coin: Coin::dummy(100000000 + i as u64),
            })
            .collect();

        Self { txid, outputs }
    }
}

fn new_test_ext() -> TestExternalities {
    let storage = frame_system::GenesisConfig::<Test>::default()
        .build_storage()
        .unwrap();
    TestExternalities::new(storage)
}

/// Populate the trie with many UTXOs to simulate real chain state
fn populate_double_map(ext: &mut TestExternalities, tx_count: usize, avg_outputs: usize) {
    ext.execute_with(|| {
        for i in 0..tx_count {
            let tx = BenchTx::new(i as u32 + 1000000, avg_outputs);
            for output in &tx.outputs {
                pallet_double_map::Coins::<Test>::insert(&tx.txid, output.vout, &output.coin);
            }
        }
    });
    ext.commit_all().unwrap();
}

/// Populate the trie with bounded vec storage
fn populate_bounded_vec(ext: &mut TestExternalities, tx_count: usize, avg_outputs: usize) {
    ext.execute_with(|| {
        for i in 0..tx_count {
            let tx = BenchTx::new(i as u32 + 1000000, avg_outputs);
            let bounded: BoundedVec<Output, MaxOutputs> = tx.outputs.try_into().unwrap();
            pallet_bounded_vec::Outputs::<Test>::insert(&tx.txid, bounded);
        }
    });
    ext.commit_all().unwrap();
}

fn bench_insert_outputs(c: &mut Criterion) {
    let mut group = c.benchmark_group("insert_outputs");

    for output_count in [1, 5, 10, 20, 50, 100] {
        let tx_count = 100;
        group.throughput(Throughput::Elements((tx_count * output_count) as u64));

        // StorageDoubleMap approach
        group.bench_with_input(
            BenchmarkId::new("double_map", output_count),
            &output_count,
            |b, &output_count| {
                b.iter_batched(
                    || {
                        let txs: Vec<_> = (0..tx_count)
                            .map(|i| BenchTx::new(i, output_count as usize))
                            .collect();
                        let mut ext = new_test_ext();
                        populate_double_map(&mut ext, 10000, 10);
                        (ext, txs)
                    },
                    |(mut ext, txs)| {
                        ext.execute_with(|| {
                            for tx in &txs {
                                for output in &tx.outputs {
                                    pallet_double_map::Coins::<Test>::insert(
                                        black_box(&tx.txid),
                                        black_box(output.vout),
                                        black_box(&output.coin),
                                    );
                                }
                            }
                            // Force trie calculation
                            let _ = sp_io::storage::root(sp_core::storage::StateVersion::V1);
                        });
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );

        // StorageMap<BoundedVec> approach
        group.bench_with_input(
            BenchmarkId::new("bounded_vec", output_count),
            &output_count,
            |b, &output_count| {
                b.iter_batched(
                    || {
                        let txs: Vec<_> = (0..tx_count)
                            .map(|i| BenchTx::new(i, output_count as usize))
                            .collect();
                        let mut ext = new_test_ext();
                        populate_bounded_vec(&mut ext, 10000, 10);
                        (ext, txs)
                    },
                    |(mut ext, txs)| {
                        ext.execute_with(|| {
                            for tx in &txs {
                                let bounded: BoundedVec<Output, MaxOutputs> =
                                    tx.outputs.clone().try_into().unwrap();
                                pallet_bounded_vec::Outputs::<Test>::insert(
                                    black_box(&tx.txid),
                                    black_box(bounded),
                                );
                            }
                            // Force trie calculation
                            let _ = sp_io::storage::root(sp_core::storage::StateVersion::V1);
                        });
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }
    group.finish();
}

fn bench_read_utxo(c: &mut Criterion) {
    let mut group = c.benchmark_group("read_utxo");
    let output_count = 10;
    let tx_count = 100;

    group.throughput(Throughput::Elements(tx_count as u64));

    // StorageDoubleMap approach
    group.bench_function("double_map", |b| {
        b.iter_batched(
            || {
                let mut ext = new_test_ext();
                let txs: Vec<_> = (0..tx_count)
                    .map(|i| BenchTx::new(i, output_count))
                    .collect();
                ext.execute_with(|| {
                    for tx in &txs {
                        for output in &tx.outputs {
                            pallet_double_map::Coins::<Test>::insert(
                                &tx.txid,
                                output.vout,
                                &output.coin,
                            );
                        }
                    }
                });
                ext.commit_all().unwrap();
                (ext, txs)
            },
            |(mut ext, txs)| {
                ext.execute_with(|| {
                    for tx in &txs {
                        // Read first output of each tx
                        let _coin = pallet_double_map::Coins::<Test>::get(
                            black_box(&tx.txid),
                            black_box(0),
                        );
                    }
                });
            },
            criterion::BatchSize::SmallInput,
        );
    });

    // StorageMap<BoundedVec> approach (must decode entire vec to get one output)
    group.bench_function("bounded_vec", |b| {
        b.iter_batched(
            || {
                let mut ext = new_test_ext();
                let txs: Vec<_> = (0..tx_count)
                    .map(|i| BenchTx::new(i, output_count))
                    .collect();
                ext.execute_with(|| {
                    for tx in &txs {
                        let bounded: BoundedVec<Output, MaxOutputs> =
                            tx.outputs.clone().try_into().unwrap();
                        pallet_bounded_vec::Outputs::<Test>::insert(&tx.txid, bounded);
                    }
                });
                ext.commit_all().unwrap();
                (ext, txs)
            },
            |(mut ext, txs)| {
                ext.execute_with(|| {
                    for tx in &txs {
                        // Must decode entire vec
                        let outputs = pallet_bounded_vec::Outputs::<Test>::get(black_box(&tx.txid));
                        // Access first output
                        let _output = outputs.first();
                    }
                });
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.finish();
}

fn bench_spend_utxo(c: &mut Criterion) {
    let mut group = c.benchmark_group("spend_utxo");
    let output_count = 10;
    let tx_count = 100;

    group.throughput(Throughput::Elements(tx_count as u64));

    // StorageDoubleMap approach (direct delete)
    group.bench_function("double_map", |b| {
        b.iter_batched(
            || {
                let mut ext = new_test_ext();
                let txs: Vec<_> = (0..tx_count)
                    .map(|i| BenchTx::new(i, output_count))
                    .collect();
                ext.execute_with(|| {
                    for tx in &txs {
                        for output in &tx.outputs {
                            pallet_double_map::Coins::<Test>::insert(
                                &tx.txid,
                                output.vout,
                                &output.coin,
                            );
                        }
                    }
                });
                ext.commit_all().unwrap();
                (ext, txs)
            },
            |(mut ext, txs)| {
                ext.execute_with(|| {
                    for tx in &txs {
                        // Direct delete - just one trie operation
                        pallet_double_map::Coins::<Test>::remove(black_box(&tx.txid), black_box(0));
                    }
                    let _ = sp_io::storage::root(sp_core::storage::StateVersion::V1);
                });
            },
            criterion::BatchSize::SmallInput,
        );
    });

    // StorageMap<BoundedVec> approach (read-modify-write)
    group.bench_function("bounded_vec", |b| {
        b.iter_batched(
            || {
                let mut ext = new_test_ext();
                let txs: Vec<_> = (0..tx_count)
                    .map(|i| BenchTx::new(i, output_count))
                    .collect();
                ext.execute_with(|| {
                    for tx in &txs {
                        let bounded: BoundedVec<Output, MaxOutputs> =
                            tx.outputs.clone().try_into().unwrap();
                        pallet_bounded_vec::Outputs::<Test>::insert(&tx.txid, bounded);
                    }
                });
                ext.commit_all().unwrap();
                (ext, txs)
            },
            |(mut ext, txs)| {
                ext.execute_with(|| {
                    for tx in &txs {
                        // Read entire vec, modify, write back - remove if empty
                        let txid = black_box(&tx.txid);
                        let should_remove =
                            pallet_bounded_vec::Outputs::<Test>::mutate(txid, |outputs| {
                                outputs.retain(|o| o.vout != 0);
                                outputs.is_empty()
                            });
                        if should_remove {
                            pallet_bounded_vec::Outputs::<Test>::remove(txid);
                        }
                    }
                    let _ = sp_io::storage::root(sp_core::storage::StateVersion::V1);
                });
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.finish();
}

fn bench_block_processing(c: &mut Criterion) {
    let mut group = c.benchmark_group("block_processing");
    let tx_count = 2000;
    let avg_outputs = 2;

    group.throughput(Throughput::Elements(tx_count as u64));
    group.sample_size(10); // Fewer samples for this heavy benchmark

    // StorageDoubleMap approach
    group.bench_function("double_map", |b| {
        b.iter_batched(
            || {
                let mut ext = new_test_ext();
                // Pre-populate with previous blocks' UTXOs
                populate_double_map(&mut ext, 50000, avg_outputs);
                ext
            },
            |mut ext| {
                ext.execute_with(|| {
                    // Simulate realistic block processing
                    for i in 0..tx_count {
                        let tx = BenchTx::new(i as u32, avg_outputs);

                        // Spend previous UTXOs (if not coinbase)
                        if i > 0 && i % 2 == 0 {
                            let prev_tx = BenchTx::new((i - 1) as u32, avg_outputs);
                            pallet_double_map::Coins::<Test>::remove(&prev_tx.txid, 0);
                        }

                        // Add new outputs
                        for output in &tx.outputs {
                            pallet_double_map::Coins::<Test>::insert(
                                &tx.txid,
                                output.vout,
                                &output.coin,
                            );
                        }
                    }
                    let _ = sp_io::storage::root(sp_core::storage::StateVersion::V1);
                });
            },
            criterion::BatchSize::SmallInput,
        );
    });

    // StorageMap<BoundedVec> approach
    group.bench_function("bounded_vec", |b| {
        b.iter_batched(
            || {
                let mut ext = new_test_ext();
                // Pre-populate with previous blocks' UTXOs
                populate_bounded_vec(&mut ext, 50000, avg_outputs);
                ext
            },
            |mut ext| {
                ext.execute_with(|| {
                    // Simulate realistic block processing
                    for i in 0..tx_count {
                        let tx = BenchTx::new(i as u32, avg_outputs);

                        // Spend previous UTXOs (read-modify-write - remove if empty)
                        if i > 0 && i % 2 == 0 {
                            let prev_tx = BenchTx::new((i - 1) as u32, avg_outputs);
                            let should_remove = pallet_bounded_vec::Outputs::<Test>::mutate(
                                &prev_tx.txid,
                                |outputs| {
                                    outputs.retain(|o| o.vout != 0);
                                    outputs.is_empty()
                                },
                            );
                            if should_remove {
                                pallet_bounded_vec::Outputs::<Test>::remove(&prev_tx.txid);
                            }
                        }

                        // Add new outputs
                        let bounded: BoundedVec<Output, MaxOutputs> =
                            tx.outputs.clone().try_into().unwrap();
                        pallet_bounded_vec::Outputs::<Test>::insert(&tx.txid, bounded);
                    }
                    let _ = sp_io::storage::root(sp_core::storage::StateVersion::V1);
                });
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_insert_outputs,
    bench_read_utxo,
    bench_spend_utxo,
    bench_block_processing
);
criterion_main!(benches);
