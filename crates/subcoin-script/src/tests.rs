pub mod interpreter;

use crate::num::NumError;
use crate::signature_checker::SECP;
use crate::stack::StackError;
use crate::{verify_script, EcdsaSignature, Error, TransactionSignatureChecker, VerifyFlags};
use bitcoin::consensus::encode::deserialize;
use bitcoin::secp256k1::Message;
use bitcoin::{PublicKey, Script, ScriptBuf, Transaction};

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
    let sig = EcdsaSignature::parse_der_lax(&hex::decode(full_sig).unwrap()).unwrap();
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
    let input = &tx.input[input_index];
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
fn test_signature_from_der_lax() {
    let _ = sc_tracing::logging::LoggerBuilder::new("subcoin_script=debug").init();

    // Block 110300:16:0
    // https://www.blockchain.com/explorer/transactions/btc/c99c49da4c38af669dea436d3e73780dfdb6c1ecf9958baa52960e8baee30e73
    let tx = "01000000012316aac445c13ff31af5f3d1e2cebcada83e54ba10d15e01f49ec28bddc285aa000000008e4b3048022200002b83d59c1d23c08efd82ee0662fec23309c3adbcbd1f0b8695378db4b14e736602220000334a96676e58b1bb01784cb7c556dd8ce1c220171904da22e18fe1e7d1510db5014104d0fe07ff74c9ef5b00fed1104fad43ecf72dbab9e60733e4f56eacf24b20cf3b8cd945bcabcc73ba0158bf9ce769d43e94bd58c5c7e331a188922b3fe9ca1f5affffffff01c0c62d00000000001976a9147a2a3b481ca80c4ba7939c54d9278e50189d94f988ac00000000";
    let tx = decode_raw_tx(tx);

    let data = hex::decode("76a9147a2a3b481ca80c4ba7939c54d9278e50189d94f988ac").unwrap();
    let script_pubkey = Script::from_bytes(&data);

    let input_index = 0;
    let input = &tx.input[input_index];
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
    .expect("Verify script from 110300:16:0");

    // Block 140493:82:0
    // https://www.blockchain.com/explorer/transactions/btc/70f7c15c6f62139cc41afa858894650344eda9975b46656d893ee59df8914a3d
    let tx = "0100000001289eb02e8ddc1ee3486aadc1cd1335fba22a8e3e87e3f41b7c5bbe7fb4391d81010000008a47304402206b5c3b1c86748dcf328b9f3a65e10085afcf5d1af5b40970d8ce3a9355e06b5b0220cdbdc23e6d3618e47056fccc60c5f73d1a542186705197e5791e97f0e6582a32014104f25ec495fa21ad14d69f45bf277129488cfb1a339aba1fed3c5099bb6d8e9716491a14050fbc0b2fed2963dc1e56264b3adf52a81b953222a2180d48b54d1e18ffffffff0140420f00000000001976a914e6ba8cc407375ce1623ec17b2f1a59f2503afc6788ac00000000";
    let tx = decode_raw_tx(tx);

    let data = hex::decode("76a914c62301ef02dfeec757fb8dedb8a45eda5fb5ee4d88ac").unwrap();
    let script_pubkey = Script::from_bytes(&data);

    let input_index = 0;
    let input = &tx.input[input_index];
    let input_amount = 1000000u64;
    let mut checker = TransactionSignatureChecker::new(&tx, input_index, input_amount);
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    verify_script(
        &input.script_sig,
        &script_pubkey,
        &input.witness,
        &flags,
        &mut checker,
    )
    .expect("Verify script from 140493:82:0");
}

#[test]
fn test_transaction_bip65() {
    let _ = sc_tracing::logging::LoggerBuilder::new("subcoin_script=debug").init();

    // https://www.blockchain.com/explorer/transactions/btc/eb3b82c0884e3efa6d8b0be55b4915eb20be124c9766245bcc7f34fdac32bccb
    let tx = "01000000024de8b0c4c2582db95fa6b3567a989b664484c7ad6672c85a3da413773e63fdb8000000006b48304502205b282fbc9b064f3bc823a23edcc0048cbb174754e7aa742e3c9f483ebe02911c022100e4b0b3a117d36cab5a67404dddbf43db7bea3c1530e0fe128ebc15621bd69a3b0121035aa98d5f77cd9a2d88710e6fc66212aff820026f0dad8f32d1f7ce87457dde50ffffffff4de8b0c4c2582db95fa6b3567a989b664484c7ad6672c85a3da413773e63fdb8010000006f004730440220276d6dad3defa37b5f81add3992d510d2f44a317fd85e04f93a1e2daea64660202200f862a0da684249322ceb8ed842fb8c859c0cb94c81e1c5308b4868157a428ee01ab51210232abdc893e7f0631364d7fd01cb33d24da45329a00357b3a7886211ab414d55a51aeffffffff02e0fd1c00000000001976a914380cb3c594de4e7e9b8e18db182987bebb5a4f7088acc0c62d000000000017142a9bc5447d664c1d0141392a842d23dba45c4f13b17500000000";
    let tx = decode_raw_tx(tx);

    let data = hex::decode("142a9bc5447d664c1d0141392a842d23dba45c4f13b175").unwrap();
    let script_pubkey = Script::from_bytes(&data);

    let input_index = 1;
    let input = &tx.input[input_index];
    let input_amount = 3000000u64;
    let mut checker = TransactionSignatureChecker::new(&tx, input_index, input_amount);

    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        verify_script(
            &input.script_sig,
            &script_pubkey,
            &input.witness,
            &flags,
            &mut checker,
        ),
        Ok(())
    );

    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS | VerifyFlags::CHECKLOCKTIMEVERIFY;

    assert_eq!(
        verify_script(
            &input.script_sig,
            &script_pubkey,
            &input.witness,
            &flags,
            &mut checker,
        ),
        Err(Error::Stack(StackError::Num(NumError::Overflow)))
    );
}

#[test]
fn test_invalid_signature_may_be_expected() {
    let _ = sc_tracing::logging::LoggerBuilder::new("subcoin_script=debug").init();

    // https://www.blockchain.com/explorer/transactions/btc/bc179baab547b7d7c1d5d8d6f8b0cc6318eaa4b0dd0a093ad6ac7f5a1cb6b3ba
    let tx = "010000000290c5e425bfba62bd5b294af0414d8fa3ed580c5ca6f351ccc23e360b14ff7f470100000091004730440220739d9ab2c3e7089e7bd311f267a65dc0ea00f49619cb61ec016a5038016ed71202201b88257809b623d471e429787c36e0a9bcd2a058fc0c75fd9c25f905657e3b9e01ab512103c86390eb5230237f31de1f02e70ce61e77f6dbfefa7d0e4ed4f6b3f78f85d8ec2103193f28067b502b34cac9eae39f74dba4815e1278bab31516efb29bd8de2c1bea52aeffffffffdd7f3ce640a2fb04dbe24630aa06e4299fbb1d3fe585fe4f80be4a96b5ff0a0d01000000b400483045022100a28d2ace2f1cb4b2a58d26a5f1a2cc15cdd4cf1c65cee8e4521971c7dc60021c0220476a5ad62bfa7c18f9174d9e5e29bc0062df543e2c336ae2c77507e462bbf95701ab512103c86390eb5230237f31de1f02e70ce61e77f6dbfefa7d0e4ed4f6b3f78f85d8ec2103193f28067b502b34cac9eae39f74dba4815e1278bab31516efb29bd8de2c1bea21032462c60ebc21f4d38b3c4ccb33be77b57ae72762be12887252db18fd6225befb53aeffffffff02e0fd1c00000000001976a9148501106ab5492387998252403d70857acfa1586488ac50c3000000000000171499050637f553f03cc0f82bbfe98dc99f10526311b17500000000";
    let tx = decode_raw_tx(tx);

    let data = hex::decode("1464d63d835705618da2111ca3194f22d067187cf2b175").unwrap();
    let script_pubkey = Script::from_bytes(&data);

    let input_index = 0;
    let input = &tx.input[input_index];
    let input_amount = 1000000u64;
    let mut checker = TransactionSignatureChecker::new(&tx, input_index, input_amount);

    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;

    assert_eq!(
        verify_script(
            &input.script_sig,
            &script_pubkey,
            &input.witness,
            &flags,
            &mut checker,
        ),
        Ok(())
    );
}
