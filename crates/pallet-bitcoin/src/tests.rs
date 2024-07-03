use bitcoin::consensus::Encodable;
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
    assert_eq!(d, runtime_txid.encode());
}
