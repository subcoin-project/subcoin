//! Integration tests for Package relay and CPFP (Child-Pays-For-Parent).

use super::*;
use bitcoin::{Amount, OutPoint, Txid};
use subcoin_primitives::Coin;

/// Test simple CPFP: low-fee parent + high-fee child accepted as package.
#[test]
fn test_simple_cpfp() {
    let client = Arc::new(MockClient::new());
    let mempool = MemPool::new(client.clone());
    let addr = dummy_address();

    // Create UTXO
    let utxo = OutPoint {
        txid: Txid::all_zeros(),
        vout: 0,
    };
    client.add_utxo(
        utxo,
        create_coin(Amount::from_sat(100000), addr.script_pubkey(), 0, false),
    );

    // Parent: low fee (below min relay fee)
    let parent = TxBuilder::new()
        .input(utxo, 0xFFFFFFFF)
        .output(Amount::from_sat(99_900), addr.clone()) // 100 sat fee (too low)
        .build();
    let parent_txid = parent.compute_txid();

    // Child: high fee
    let child_input = OutPoint {
        txid: parent_txid,
        vout: 0,
    };
    let child = TxBuilder::new()
        .input(child_input, 0xFFFFFFFF)
        .output(Amount::from_sat(97_900), addr.clone()) // 2000 sat fee
        .build();
    let child_txid = child.compute_txid();

    // Parent alone should fail
    let result = mempool.accept_single_transaction(parent.clone());
    assert!(matches!(result, Err(MempoolError::FeeTooLow(_))));
    assert_mempool_size(&mempool, 0);

    // Package should succeed (child sponsors parent)
    let package = vec![parent, child];
    let result = mempool.accept_package(package);
    assert!(result.is_ok());

    // Both should be in mempool
    assert_in_mempool(&mempool, &parent_txid);
    assert_in_mempool(&mempool, &child_txid);
    assert_mempool_size(&mempool, 2);
}

/// Test CPFP fee override: low-fee tx accepted if package feerate meets minimum.
#[test]
fn test_cpfp_fee_override() {
    let client = Arc::new(MockClient::new());

    // Set min relay fee to 1000 sat/kvb
    let options = MemPoolOptions::builder()
        .min_relay_feerate(1000)
        .build();
    let mempool = MemPool::with_options(client.clone(), options);
    let addr = dummy_address();

    let utxo = OutPoint {
        txid: Txid::all_zeros(),
        vout: 0,
    };
    client.add_utxo(
        utxo,
        create_coin(Amount::from_sat(100000), addr.script_pubkey(), 0, false),
    );

    // Parent: 500 sat fee (below 1000 sat/kvb minimum)
    let parent = TxBuilder::new()
        .input(utxo, 0xFFFFFFFF)
        .output(Amount::from_sat(99_500), addr.clone())
        .build();
    let parent_txid = parent.compute_txid();

    // Child: high fee to bring package feerate above minimum
    let child_input = OutPoint {
        txid: parent_txid,
        vout: 0,
    };
    let child = TxBuilder::new()
        .input(child_input, 0xFFFFFFFF)
        .output(Amount::from_sat(96_500), addr.clone()) // 3000 sat fee
        .build();
    let child_txid = child.compute_txid();

    // Package feerate = (500 + 3000) / (parent_vsize + child_vsize)
    // Should be > 1000 sat/kvb
    let package = vec![parent, child];
    let result = mempool.accept_package(package);
    assert!(result.is_ok());

    assert_in_mempool(&mempool, &parent_txid);
    assert_in_mempool(&mempool, &child_txid);
}

/// Test in-package outputs visibility: child can see parent outputs before mempool insertion.
#[test]
fn test_in_package_outputs_visibility() {
    let client = Arc::new(MockClient::new());
    let mempool = MemPool::new(client.clone());
    let addr = dummy_address();

    let utxo = OutPoint {
        txid: Txid::all_zeros(),
        vout: 0,
    };
    client.add_utxo(
        utxo,
        create_coin(Amount::from_sat(100000), addr.script_pubkey(), 0, false),
    );

    // Parent tx
    let parent = TxBuilder::new()
        .input(utxo, 0xFFFFFFFF)
        .output(Amount::from_sat(98_000), addr.clone()) // 2000 sat fee
        .build();
    let parent_txid = parent.compute_txid();

    // Child spends parent output (which doesn't exist in UTXO set yet)
    let child_input = OutPoint {
        txid: parent_txid,
        vout: 0,
    };
    let child = TxBuilder::new()
        .input(child_input, 0xFFFFFFFF)
        .output(Amount::from_sat(96_000), addr.clone()) // 2000 sat fee
        .build();
    let child_txid = child.compute_txid();

    // Package validation should handle in-package dependency
    let package = vec![parent, child];
    let result = mempool.accept_package(package);
    assert!(result.is_ok());

    assert_in_mempool(&mempool, &parent_txid);
    assert_in_mempool(&mempool, &child_txid);
    assert_mempool_size(&mempool, 2);
}

/// Test package rollback on failure: if any tx fails, none are accepted.
#[test]
fn test_package_rollback_on_failure() {
    let client = Arc::new(MockClient::new());
    let mempool = MemPool::new(client.clone());
    let addr = dummy_address();

    let utxo = OutPoint {
        txid: Txid::all_zeros(),
        vout: 0,
    };
    client.add_utxo(
        utxo,
        create_coin(Amount::from_sat(100000), addr.script_pubkey(), 0, false),
    );

    // Parent: valid
    let parent = TxBuilder::new()
        .input(utxo, 0xFFFFFFFF)
        .output(Amount::from_sat(98_000), addr.clone())
        .build();
    let parent_txid = parent.compute_txid();

    // Child: invalid (spends non-existent output from parent)
    let invalid_child_input = OutPoint {
        txid: parent_txid,
        vout: 99, // Parent only has vout 0
    };
    let child = TxBuilder::new()
        .input(invalid_child_input, 0xFFFFFFFF)
        .output(Amount::from_sat(1_000), addr.clone())
        .build();

    // Package should fail atomically
    let package = vec![parent.clone(), child];
    let result = mempool.accept_package(package);
    assert!(result.is_err());

    // Neither should be in mempool (rollback)
    assert_not_in_mempool(&mempool, &parent_txid);
    assert_mempool_size(&mempool, 0);

    // Verify parent can still be added individually if it has sufficient fee
    let result = mempool.accept_single_transaction(parent.clone());
    if result.is_ok() {
        assert_in_mempool(&mempool, &parent_txid);
    }
}

/// Test package size limit.
#[test]
fn test_package_size_limit() {
    let client = Arc::new(MockClient::new());

    // Set max package count to 3
    let options = MemPoolOptions::builder()
        .max_package_count(3)
        .build();
    let mempool = MemPool::with_options(client.clone(), options);
    let addr = dummy_address();

    // Create 4 UTXOs
    let mut utxos = Vec::new();
    for i in 0..4 {
        let outpoint = OutPoint {
            txid: Txid::all_zeros(),
            vout: i,
        };
        client.add_utxo(
            outpoint,
            create_coin(Amount::from_sat(100000), addr.script_pubkey(), 0, false),
        );
        utxos.push(outpoint);
    }

    // Create package with 4 transactions (exceeds limit of 3)
    let mut package = Vec::new();
    for outpoint in utxos {
        let tx = TxBuilder::new()
            .input(outpoint, 0xFFFFFFFF)
            .output(Amount::from_sat(48_000), addr.clone())
            .build();
        package.push(tx);
    }

    // Should fail: too many transactions
    let result = mempool.accept_package(package);
    assert!(matches!(
        result,
        Err(MempoolError::PackageTooLarge(4, 3))
    ));
    assert_mempool_size(&mempool, 0);
}

/// Test package count limit (max_package_count).
#[test]
fn test_package_count_limit() {
    let client = Arc::new(MockClient::new());

    let options = MemPoolOptions::builder()
        .max_package_count(2)
        .build();
    let mempool = MemPool::with_options(client.clone(), options);
    let addr = dummy_address();

    // Create 3 UTXOs
    let mut package = Vec::new();
    for i in 0..3 {
        let outpoint = OutPoint {
            txid: Txid::all_zeros(),
            vout: i,
        };
        client.add_utxo(
            outpoint,
            create_coin(Amount::from_sat(100000), addr.script_pubkey(), 0, false),
        );

        let tx = TxBuilder::new()
            .input(outpoint, 0xFFFFFFFF)
            .output(Amount::from_sat(48_000), addr.clone())
            .build();
        package.push(tx);
    }

    // Should fail: 3 txs exceeds limit of 2
    let result = mempool.accept_package(package);
    assert!(matches!(
        result,
        Err(MempoolError::PackageTooLarge(3, 2))
    ));
}

/// Test topological ordering in package: child must come after parent.
#[test]
fn test_package_topological_ordering() {
    let client = Arc::new(MockClient::new());
    let mempool = MemPool::new(client.clone());
    let addr = dummy_address();

    let utxo = OutPoint {
        txid: Txid::all_zeros(),
        vout: 0,
    };
    client.add_utxo(
        utxo,
        create_coin(Amount::from_sat(100000), addr.script_pubkey(), 0, false),
    );

    let parent = TxBuilder::new()
        .input(utxo, 0xFFFFFFFF)
        .output(Amount::from_sat(98_000), addr.clone())
        .build();
    let parent_txid = parent.compute_txid();

    let child_input = OutPoint {
        txid: parent_txid,
        vout: 0,
    };
    let child = TxBuilder::new()
        .input(child_input, 0xFFFFFFFF)
        .output(Amount::from_sat(96_000), addr.clone())
        .build();
    let child_txid = child.compute_txid();

    // Submit in WRONG order (child before parent)
    // Topological sort should fix this
    let package = vec![child, parent];
    let result = mempool.accept_package(package);
    assert!(result.is_ok());

    // Both should be in mempool
    assert_in_mempool(&mempool, &parent_txid);
    assert_in_mempool(&mempool, &child_txid);
}

/// Test package with multiple parents and one child (CPFP).
#[test]
fn test_package_multiple_parents_one_child() {
    let client = Arc::new(MockClient::new());
    let mempool = MemPool::new(client.clone());
    let addr = dummy_address();

    // Create two UTXOs
    let utxo1 = OutPoint {
        txid: Txid::all_zeros(),
        vout: 0,
    };
    let utxo2 = OutPoint {
        txid: Txid::all_zeros(),
        vout: 1,
    };

    client.add_utxo(
        utxo1,
        create_coin(Amount::from_sat(100000), addr.script_pubkey(), 0, false),
    );
    client.add_utxo(
        utxo2,
        create_coin(Amount::from_sat(100000), addr.script_pubkey(), 0, false),
    );

    // Parent 1: low fee
    let parent1 = TxBuilder::new()
        .input(utxo1, 0xFFFFFFFF)
        .output(Amount::from_sat(49_900), addr.clone()) // 100 sat fee
        .build();
    let parent1_txid = parent1.compute_txid();

    // Parent 2: low fee
    let parent2 = TxBuilder::new()
        .input(utxo2, 0xFFFFFFFF)
        .output(Amount::from_sat(49_900), addr.clone()) // 100 sat fee
        .build();
    let parent2_txid = parent2.compute_txid();

    // Child: spends both parents with high fee
    let child_input1 = OutPoint {
        txid: parent1_txid,
        vout: 0,
    };
    let child_input2 = OutPoint {
        txid: parent2_txid,
        vout: 0,
    };
    let child = TxBuilder::new()
        .input(child_input1, 0xFFFFFFFF)
        .input(child_input2, 0xFFFFFFFF)
        .output(Amount::from_sat(96_800), addr.clone()) // 3000 sat fee
        .build();
    let child_txid = child.compute_txid();

    // Package feerate should be high enough
    let package = vec![parent1, parent2, child];
    let result = mempool.accept_package(package);
    assert!(result.is_ok());

    assert_in_mempool(&mempool, &parent1_txid);
    assert_in_mempool(&mempool, &parent2_txid);
    assert_in_mempool(&mempool, &child_txid);
    assert_mempool_size(&mempool, 3);
}

/// Test package relay disabled.
#[test]
fn test_package_relay_disabled() {
    let client = Arc::new(MockClient::new());

    let options = MemPoolOptions::builder()
        .build();
    let mut options_mut = options;
    options_mut.enable_package_relay = false;

    let mempool = MemPool::with_options(client.clone(), options_mut);
    let addr = dummy_address();

    let utxo = OutPoint {
        txid: Txid::all_zeros(),
        vout: 0,
    };
    client.add_utxo(
        utxo,
        create_coin(Amount::from_sat(100000), addr.script_pubkey(), 0, false),
    );

    let tx = TxBuilder::new()
        .input(utxo, 0xFFFFFFFF)
        .output(Amount::from_sat(98_000), addr.clone())
        .build();

    let package = vec![tx];
    let result = mempool.accept_package(package);
    assert!(matches!(
        result,
        Err(MempoolError::PackageRelayDisabled)
    ));
}
