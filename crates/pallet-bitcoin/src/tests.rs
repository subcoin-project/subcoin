use bitcoin::consensus::{deserialize, Encodable};
use hex::test_hex_unwrap as hex;
use sp_core::Encode;

#[test]
fn test_runtime_txid_type() {
    let genesis_block = bitcoin::constants::genesis_block(bitcoin::Network::Bitcoin);
    let txid = genesis_block.txdata[0].compute_txid();

    let runtime_txid = crate::Txid::from_bitcoin_txid(txid);
    assert_eq!(format!("{txid:?}"), format!("{runtime_txid:?}"));

    let mut d = Vec::new();
    txid.consensus_encode(&mut d)
        .expect("txid must be encoded correctly; qed");
    d.reverse();
    assert_eq!(d, runtime_txid.encode());
}

#[test]
fn test_runtime_transaction_type() {
    let tx_bytes = hex!(
        "02000000000101595895ea20179de87052b4046dfe6fd515860505d6511a9004cf12a1f93cac7c01000000\
            00ffffffff01deb807000000000017a9140f3444e271620c736808aa7b33e370bd87cb5a078702483045022\
            100fb60dad8df4af2841adc0346638c16d0b8035f5e3f3753b88db122e70c79f9370220756e6633b17fd271\
            0e626347d28d60b0a2d6cbb41de51740644b9fb3ba7751040121028fa937ca8cba2197a37c007176ed89410\
            55d3bcb8627d085e94553e62f057dcc00000000"
    );
    let tx: bitcoin::Transaction = deserialize(&tx_bytes).unwrap();

    let runtime_tx: crate::types::Transaction = tx.clone().into();

    assert_eq!(tx, bitcoin::Transaction::from(runtime_tx));
}
