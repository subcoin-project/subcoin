pub mod interpreter;

use crate::signature_checker::SECP;
use crate::{
    verify_script, EcdsaSignature, NoSignatureCheck, TransactionSignatureChecker, VerifyFlags,
};
use bitcoin::consensus::encode::deserialize;
use bitcoin::secp256k1::Message;
use bitcoin::sighash::SighashCache;
use bitcoin::{EcdsaSighashType, PublicKey, Script, ScriptBuf, Transaction};

#[test]
fn test_basic_p2pk() {
    let _ = sc_tracing::logging::LoggerBuilder::new("subcoin_script=debug").init();

    // https://www.blockchain.com/explorer/transactions/btc/12b5633bad1f9c167d523ad1aa1947b2732a865bf5414eab2f9e5ae5d5c191ba
    let tx = "010000000173805864da01f15093f7837607ab8be7c3705e29a9d4a12c9116d709f8911e590100000049483045022052ffc1929a2d8bd365c6a2a4e3421711b4b1e1b8781698ca9075807b4227abcb0221009984107ddb9e3813782b095d0d84361ed4c76e5edaf6561d252ae162c2341cfb01ffffffff0200e1f50500000000434104baa9d36653155627c740b3409a734d4eaf5dcca9fb4f736622ee18efcf0aec2b758b2ec40db18fbae708f691edb2d4a2a3775eb413d16e2e3c0f8d4c69119fd1ac009ce4a60000000043410411db93e1dcdb8a016b49840f8c53bc1eb68a382e97b1482ecad7b148a6909a5cb2e0eaddfb84ccf9744464f82e160bfa9b8b64f9d4c03f999b8643f656b412a3ac00000000";
    let tx_data = hex::decode(tx).unwrap();
    let tx: Transaction = deserialize(&tx_data).unwrap();

    let pubkey_data = "0411db93e1dcdb8a016b49840f8c53bc1eb68a382e97b1482ecad7b148a6909a5cb2e0eaddfb84ccf9744464f82e160bfa9b8b64f9d4c03f999b8643f656b412a3";
    let pubkey = PublicKey::from_slice(&hex::decode(pubkey_data).unwrap()).unwrap();

    let script_pubkey = ScriptBuf::new_p2pk(&pubkey);

    let sighash_cache = SighashCache::new(&tx);

    let sighash = sighash_cache
        .legacy_signature_hash(0, &script_pubkey, EcdsaSighashType::All.to_u32())
        .unwrap();
    println!("xxxxxxxx sihash: {:?}", sighash);

    for (input_index, input) in tx.input.iter().enumerate() {
        // input_amount is irrelevant here.
        let input_amount = 0u64;
        let mut checker = TransactionSignatureChecker::new(input_index, input_amount, &tx);
        let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
        verify_script(
            &input.script_sig,
            &script_pubkey,
            &input.witness,
            flags,
            &mut checker,
        )
        .expect("Verify p2pk");
    }
}

#[test]
fn test_verify_ecdsa_signature() {
    // msg is collected from Block 183, tx_index=1, input_index=0
    let msg = "1be470440ffae5a7f2bc59862007402e32f7d04b43311543006200e470f0d606";
    let pk = "0411db93e1dcdb8a016b49840f8c53bc1eb68a382e97b1482ecad7b148a6909a5cb2e0eaddfb84ccf9744464f82e160bfa9b8b64f9d4c03f999b8643f656b412a3";
    let full_sig = "3045022052ffc1929a2d8bd365c6a2a4e3421711b4b1e1b8781698ca9075807b4227abcb0221009984107ddb9e3813782b095d0d84361ed4c76e5edaf6561d252ae162c2341cfb01";

    let pk = PublicKey::from_slice(&hex::decode(pk).unwrap()).unwrap();
    let msg = Message::from_digest_slice(&hex::decode(msg).unwrap()).unwrap();

    let mut sig = EcdsaSignature::from_slice(&hex::decode(full_sig).unwrap()).unwrap();
    // Ensure normalize_s() is invoked.
    sig.signature.normalize_s();

    pk.verify(&SECP, &msg, &sig).unwrap();
}
