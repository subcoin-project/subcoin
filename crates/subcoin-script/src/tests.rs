pub mod interpreter;

use crate::signature_checker::SECP;
use crate::{verify_script, EcdsaSignature, TransactionSignatureChecker, VerifyFlags};
use bitcoin::consensus::encode::deserialize;
use bitcoin::secp256k1::Message;
use bitcoin::{EcdsaSighashType, PublicKey, Script, ScriptBuf, Transaction};

fn decode_raw_tx(tx_hex: &str) -> Transaction {
    let tx_data = hex::decode(tx_hex).unwrap();
    deserialize(&tx_data).unwrap()
}

fn decode_pubkey(pubkey_hex: &str) -> PublicKey {
    PublicKey::from_slice(&hex::decode(pubkey_hex).unwrap()).unwrap()
}

#[test]
fn test_basic_p2pk() {
    let _ = sc_tracing::logging::LoggerBuilder::new("subcoin_script=debug").init();

    // https://www.blockchain.com/explorer/transactions/btc/12b5633bad1f9c167d523ad1aa1947b2732a865bf5414eab2f9e5ae5d5c191ba
    let tx = "010000000173805864da01f15093f7837607ab8be7c3705e29a9d4a12c9116d709f8911e590100000049483045022052ffc1929a2d8bd365c6a2a4e3421711b4b1e1b8781698ca9075807b4227abcb0221009984107ddb9e3813782b095d0d84361ed4c76e5edaf6561d252ae162c2341cfb01ffffffff0200e1f50500000000434104baa9d36653155627c740b3409a734d4eaf5dcca9fb4f736622ee18efcf0aec2b758b2ec40db18fbae708f691edb2d4a2a3775eb413d16e2e3c0f8d4c69119fd1ac009ce4a60000000043410411db93e1dcdb8a016b49840f8c53bc1eb68a382e97b1482ecad7b148a6909a5cb2e0eaddfb84ccf9744464f82e160bfa9b8b64f9d4c03f999b8643f656b412a3ac00000000";
    let tx = decode_raw_tx(tx);

    let pubkey = "0411db93e1dcdb8a016b49840f8c53bc1eb68a382e97b1482ecad7b148a6909a5cb2e0eaddfb84ccf9744464f82e160bfa9b8b64f9d4c03f999b8643f656b412a3";
    let pubkey = decode_pubkey(pubkey);

    let script_pubkey = ScriptBuf::new_p2pk(&pubkey);

    for (input_index, input) in tx.input.iter().enumerate() {
        // input_amount is irrelevant here.
        let input_amount = 0u64;
        let mut checker = TransactionSignatureChecker::new(&tx, input_index, input_amount);
        let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
        verify_script(
            &input.script_sig,
            &script_pubkey,
            &input.witness,
            &flags,
            &mut checker,
        )
        .expect("Verify p2pk");
    }
}

#[test]
fn test_ecdsa_signature_normalize_s() {
    // msg is collected from Block 183, tx_index=1, input_index=0
    let msg = "1be470440ffae5a7f2bc59862007402e32f7d04b43311543006200e470f0d606";
    let pk = "0411db93e1dcdb8a016b49840f8c53bc1eb68a382e97b1482ecad7b148a6909a5cb2e0eaddfb84ccf9744464f82e160bfa9b8b64f9d4c03f999b8643f656b412a3";
    let full_sig = "3045022052ffc1929a2d8bd365c6a2a4e3421711b4b1e1b8781698ca9075807b4227abcb0221009984107ddb9e3813782b095d0d84361ed4c76e5edaf6561d252ae162c2341cfb01";

    let pk = PublicKey::from_slice(&hex::decode(pk).unwrap()).unwrap();
    let msg = Message::from_digest_slice(&hex::decode(msg).unwrap()).unwrap();
    let sig = EcdsaSignature::from_slice(&hex::decode(full_sig).unwrap()).unwrap();
    SECP.verify_ecdsa(&msg, &sig.signature, &pk.inner)
        .expect("Verify ECDSA signature");
}

#[test]
fn test_sig_with_sighash_type_0() {
    let _ = sc_tracing::logging::LoggerBuilder::new("subcoin_script=debug").init();

    // Block 110300:16:0
    // https://www.blockchain.com/explorer/transactions/btc/c99c49da4c38af669dea436d3e73780dfdb6c1ecf9958baa52960e8baee30e73
    let tx = "01000000010276b76b07f4935c70acf54fbf1f438a4c397a9fb7e633873c4dd3bc062b6b40000000008c493046022100d23459d03ed7e9511a47d13292d3430a04627de6235b6e51a40f9cd386f2abe3022100e7d25b080f0bb8d8d5f878bba7d54ad2fda650ea8d158a33ee3cbd11768191fd004104b0e2c879e4daf7b9ab68350228c159766676a14f5815084ba166432aab46198d4cca98fa3e9981d0a90b2effc514b76279476550ba3663fdcaff94c38420e9d5000000000100093d00000000001976a9149a7b0f3b80c6baaeedce0a0842553800f832ba1f88ac00000000";
    let tx = decode_raw_tx(tx);

    let data = hex::decode("76a914dc44b1164188067c3a32d4780f5996fa14a4f2d988ac").unwrap();
    let script_pubkey = Script::from_bytes(&data);

    let input_index = 0;
    let input = tx.input[0].clone();
    let input_amount = 5000000u64;
    let mut checker = TransactionSignatureChecker::new(&tx, input_index, input_amount);
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    verify_script(
        &input.script_sig,
        &script_pubkey,
        &input.witness,
        &flags,
        &mut checker,
    )
    .expect("Verify script with sighash_type is 0");
}

#[test]
fn test_signature_from_der_lax_for_early_blocks() {
    let _ = sc_tracing::logging::LoggerBuilder::new("subcoin_script=debug").init();

    // Block 110300:16:0
    // https://www.blockchain.com/explorer/transactions/btc/c99c49da4c38af669dea436d3e73780dfdb6c1ecf9958baa52960e8baee30e73
    let tx = "01000000012316aac445c13ff31af5f3d1e2cebcada83e54ba10d15e01f49ec28bddc285aa000000008e4b3048022200002b83d59c1d23c08efd82ee0662fec23309c3adbcbd1f0b8695378db4b14e736602220000334a96676e58b1bb01784cb7c556dd8ce1c220171904da22e18fe1e7d1510db5014104d0fe07ff74c9ef5b00fed1104fad43ecf72dbab9e60733e4f56eacf24b20cf3b8cd945bcabcc73ba0158bf9ce769d43e94bd58c5c7e331a188922b3fe9ca1f5affffffff01c0c62d00000000001976a9147a2a3b481ca80c4ba7939c54d9278e50189d94f988ac00000000";
    let tx = decode_raw_tx(tx);

    let data = hex::decode("76a9147a2a3b481ca80c4ba7939c54d9278e50189d94f988ac").unwrap();
    let script_pubkey = Script::from_bytes(&data);

    let input_index = 0;
    let input = tx.input[0].clone();
    let input_amount = 3000000u64;
    let mut checker = TransactionSignatureChecker::new(&tx, input_index, input_amount);
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    verify_script(
        &input.script_sig,
        &script_pubkey,
        &input.witness,
        &flags,
        &mut checker,
    )
    .expect("Verify script with sighash_type is 0");
}
