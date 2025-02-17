pub mod opcode;

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
        let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
        let mut checker = TransactionSignatureChecker::new(&tx, input_index, input_amount);
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
fn test_sig_with_sighash_type_0() {
    let _ = sc_tracing::logging::LoggerBuilder::new("subcoin_script=debug").init();

    // Block 110300:16:0
    // https://www.blockchain.com/explorer/transactions/btc/c99c49da4c38af669dea436d3e73780dfdb6c1ecf9958baa52960e8baee30e73
    let tx = "01000000010276b76b07f4935c70acf54fbf1f438a4c397a9fb7e633873c4dd3bc062b6b40000000008c493046022100d23459d03ed7e9511a47d13292d3430a04627de6235b6e51a40f9cd386f2abe3022100e7d25b080f0bb8d8d5f878bba7d54ad2fda650ea8d158a33ee3cbd11768191fd004104b0e2c879e4daf7b9ab68350228c159766676a14f5815084ba166432aab46198d4cca98fa3e9981d0a90b2effc514b76279476550ba3663fdcaff94c38420e9d5000000000100093d00000000001976a9149a7b0f3b80c6baaeedce0a0842553800f832ba1f88ac00000000";
    let tx = decode_raw_tx(tx);

    let data = hex::decode("76a914dc44b1164188067c3a32d4780f5996fa14a4f2d988ac").unwrap();
    let script_pubkey = Script::from_bytes(&data);

    let input_index = 0;
    let input_amount = 5000000u64;

    let input = &tx.input[input_index];
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    let mut checker = TransactionSignatureChecker::new(&tx, input_index, input_amount);

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
    let input_amount = 3000000u64;

    let input = &tx.input[input_index];
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    let mut checker = TransactionSignatureChecker::new(&tx, input_index, input_amount);

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
    let input_amount = 1000000u64;

    let input = &tx.input[input_index];
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    let mut checker = TransactionSignatureChecker::new(&tx, input_index, input_amount);

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
fn test_multisig_may_contain_invalid_signature() {
    let _ = sc_tracing::logging::LoggerBuilder::new("subcoin_script=debug").init();

    // https://www.blockchain.com/explorer/transactions/btc/bc179baab547b7d7c1d5d8d6f8b0cc6318eaa4b0dd0a093ad6ac7f5a1cb6b3ba
    let tx = "010000000290c5e425bfba62bd5b294af0414d8fa3ed580c5ca6f351ccc23e360b14ff7f470100000091004730440220739d9ab2c3e7089e7bd311f267a65dc0ea00f49619cb61ec016a5038016ed71202201b88257809b623d471e429787c36e0a9bcd2a058fc0c75fd9c25f905657e3b9e01ab512103c86390eb5230237f31de1f02e70ce61e77f6dbfefa7d0e4ed4f6b3f78f85d8ec2103193f28067b502b34cac9eae39f74dba4815e1278bab31516efb29bd8de2c1bea52aeffffffffdd7f3ce640a2fb04dbe24630aa06e4299fbb1d3fe585fe4f80be4a96b5ff0a0d01000000b400483045022100a28d2ace2f1cb4b2a58d26a5f1a2cc15cdd4cf1c65cee8e4521971c7dc60021c0220476a5ad62bfa7c18f9174d9e5e29bc0062df543e2c336ae2c77507e462bbf95701ab512103c86390eb5230237f31de1f02e70ce61e77f6dbfefa7d0e4ed4f6b3f78f85d8ec2103193f28067b502b34cac9eae39f74dba4815e1278bab31516efb29bd8de2c1bea21032462c60ebc21f4d38b3c4ccb33be77b57ae72762be12887252db18fd6225befb53aeffffffff02e0fd1c00000000001976a9148501106ab5492387998252403d70857acfa1586488ac50c3000000000000171499050637f553f03cc0f82bbfe98dc99f10526311b17500000000";
    let tx = decode_raw_tx(tx);

    let data = hex::decode("1464d63d835705618da2111ca3194f22d067187cf2b175").unwrap();
    let script_pubkey = Script::from_bytes(&data);

    let input_index = 0;
    let input_amount = 1000000u64;

    let input = &tx.input[input_index];
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    let mut checker = TransactionSignatureChecker::new(&tx, input_index, input_amount);

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

#[test]
fn test_multisig_may_contain_invalid_pubkey() {
    let _ = sc_tracing::logging::LoggerBuilder::new("subcoin_script=debug").init();

    // https://www.blockchain.com/explorer/transactions/btc/70c4e749f2b8b907875d1483ae43e8a6790b0c8397bbb33682e3602617f9a77a
    let tx = "01000000025718fb915fb8b3a802bb699ddf04dd91261ef6715f5f2820a2b1b9b7e38b4f27000000004a004830450221008c2107ed4e026ab4319a591e8d9ec37719cdea053951c660566e3a3399428af502202ecd823d5f74a77cc2159d8af2d3ea5d36a702fef9a7edaaf562aef22ac35da401ffffffff038f52231b994efb980382e4d804efeadaee13cfe01abe0d969038ccb45ec17000000000490047304402200487cd787fde9b337ab87f9fe54b9fd46d5d1692aa58e97147a4fe757f6f944202203cbcfb9c0fc4e3c453938bbea9e5ae64030cf7a97fafaf460ea2cb54ed5651b501ffffffff0100093d00000000001976a9144dc39248253538b93d3a0eb122d16882b998145888ac00000000";
    let tx = decode_raw_tx(tx);

    let pkscript = hex::decode("51210351efb6e91a31221652105d032a2508275f374cea63939ad72f1b1e02f477da782100f2b7816db49d55d24df7bdffdbc1e203b424e8cd39f5651ab938e5e4a193569e52ae").unwrap();
    let script_pubkey = Script::from_bytes(&pkscript);

    let input_index = 0;
    let input_amount = 2u64;

    let input = &tx.input[input_index];
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    let mut checker = TransactionSignatureChecker::new(&tx, input_index, input_amount);

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

#[test]
fn test_check_transaction_multisig() {
    let _ = sc_tracing::logging::LoggerBuilder::new("subcoin_script=debug").init();

    // https://www.blockchain.com/explorer/transactions/btc/02b082113e35d5386285094c2829e7e2963fa0b5369fb7f4b79c4c90877dcd3d
    let tx = "01000000013dcd7d87904c9cb7f4b79f36b5a03f96e2e729284c09856238d5353e1182b00200000000fd5e0100483045022100deeb1f13b5927b5e32d877f3c42a4b028e2e0ce5010fdb4e7f7b5e2921c1dcd2022068631cb285e8c1be9f061d2968a18c3163b780656f30a049effee640e80d9bff01483045022100ee80e164622c64507d243bd949217d666d8b16486e153ac6a1f8e04c351b71a502203691bef46236ca2b4f5e60a82a853a33d6712d6a1e7bf9a65e575aeb7328db8c014cc9524104a882d414e478039cd5b52a92ffb13dd5e6bd4515497439dffd691a0f12af9575fa349b5694ed3155b136f09e63975a1700c9f4d4df849323dac06cf3bd6458cd41046ce31db9bdd543e72fe3039a1f1c047dab87037c36a669ff90e28da1848f640de68c2fe913d363a51154a0c62d7adea1b822d05035077418267b1a1379790187410411ffd36c70776538d079fbae117dc38effafb33304af83ce4894589747aee1ef992f63280567f52f5ba870678b4ab4ff6c8ea600bd217870a8b4f1f09f3a8e8353aeffffffff0130d90000000000001976a914569076ba39fc4ff6a2291d9ea9196d8c08f9c7ab88ac00000000";
    let tx = decode_raw_tx(tx);

    let pkscript = hex::decode("a9141a8b0026343166625c7475f01e48b5ede8c0252e87").unwrap();
    let script_pubkey = Script::from_bytes(&pkscript);

    let input_index = 0;
    let input_amount = 75600u64;

    let input = &tx.input[input_index];
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    let mut checker = TransactionSignatureChecker::new(&tx, input_index, input_amount);

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

#[test]
fn test_transaction_with_high_s_signature() {
    let _ = sc_tracing::logging::LoggerBuilder::new("subcoin_script=debug").init();

    // https://www.blockchain.com/explorer/transactions/btc/12b5633bad1f9c167d523ad1aa1947b2732a865bf5414eab2f9e5ae5d5c191ba?show_adv=true
    let tx = "010000000173805864da01f15093f7837607ab8be7c3705e29a9d4a12c9116d709f8911e590100000049483045022052ffc1929a2d8bd365c6a2a4e3421711b4b1e1b8781698ca9075807b4227abcb0221009984107ddb9e3813782b095d0d84361ed4c76e5edaf6561d252ae162c2341cfb01ffffffff0200e1f50500000000434104baa9d36653155627c740b3409a734d4eaf5dcca9fb4f736622ee18efcf0aec2b758b2ec40db18fbae708f691edb2d4a2a3775eb413d16e2e3c0f8d4c69119fd1ac009ce4a60000000043410411db93e1dcdb8a016b49840f8c53bc1eb68a382e97b1482ecad7b148a6909a5cb2e0eaddfb84ccf9744464f82e160bfa9b8b64f9d4c03f999b8643f656b412a3ac00000000";
    let tx = decode_raw_tx(tx);

    let pkscript = hex::decode("410411db93e1dcdb8a016b49840f8c53bc1eb68a382e97b1482ecad7b148a6909a5cb2e0eaddfb84ccf9744464f82e160bfa9b8b64f9d4c03f999b8643f656b412a3ac").unwrap();
    let script_pubkey = Script::from_bytes(&pkscript);

    let input_index = 0;
    let input_amount = 2900000000u64;

    let input = &tx.input[input_index];
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    let mut checker = TransactionSignatureChecker::new(&tx, input_index, input_amount);

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

#[test]
fn test_transaction_from_124276() {
    let _ = sc_tracing::logging::LoggerBuilder::new("subcoin_script=debug").init();

    // https://www.blockchain.com/explorer/transactions/btc/fb0a1d8d34fa5537e461ac384bac761125e1bfa7fec286fa72511240fa66864d
    let tx = "01000000012316aac445c13ff31af5f3d1e2cebcada83e54ba10d15e01f49ec28bddc285aa000000008e4b3048022200002b83d59c1d23c08efd82ee0662fec23309c3adbcbd1f0b8695378db4b14e736602220000334a96676e58b1bb01784cb7c556dd8ce1c220171904da22e18fe1e7d1510db5014104d0fe07ff74c9ef5b00fed1104fad43ecf72dbab9e60733e4f56eacf24b20cf3b8cd945bcabcc73ba0158bf9ce769d43e94bd58c5c7e331a188922b3fe9ca1f5affffffff01c0c62d00000000001976a9147a2a3b481ca80c4ba7939c54d9278e50189d94f988ac00000000";
    let tx = decode_raw_tx(tx);

    let pkscript = hex::decode("76a9147a2a3b481ca80c4ba7939c54d9278e50189d94f988ac").unwrap();
    let script_pubkey = Script::from_bytes(&pkscript);

    let input_index = 0;
    let input_amount = 0u64;

    let input = &tx.input[input_index];
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    let mut checker = TransactionSignatureChecker::new(&tx, input_index, input_amount);

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

#[test]
fn test_transaction_bip65() {
    let _ = sc_tracing::logging::LoggerBuilder::new("subcoin_script=debug").init();

    // https://www.blockchain.com/explorer/transactions/btc/eb3b82c0884e3efa6d8b0be55b4915eb20be124c9766245bcc7f34fdac32bccb
    let tx = "01000000024de8b0c4c2582db95fa6b3567a989b664484c7ad6672c85a3da413773e63fdb8000000006b48304502205b282fbc9b064f3bc823a23edcc0048cbb174754e7aa742e3c9f483ebe02911c022100e4b0b3a117d36cab5a67404dddbf43db7bea3c1530e0fe128ebc15621bd69a3b0121035aa98d5f77cd9a2d88710e6fc66212aff820026f0dad8f32d1f7ce87457dde50ffffffff4de8b0c4c2582db95fa6b3567a989b664484c7ad6672c85a3da413773e63fdb8010000006f004730440220276d6dad3defa37b5f81add3992d510d2f44a317fd85e04f93a1e2daea64660202200f862a0da684249322ceb8ed842fb8c859c0cb94c81e1c5308b4868157a428ee01ab51210232abdc893e7f0631364d7fd01cb33d24da45329a00357b3a7886211ab414d55a51aeffffffff02e0fd1c00000000001976a914380cb3c594de4e7e9b8e18db182987bebb5a4f7088acc0c62d000000000017142a9bc5447d664c1d0141392a842d23dba45c4f13b17500000000";
    let tx = decode_raw_tx(tx);

    let data = hex::decode("142a9bc5447d664c1d0141392a842d23dba45c4f13b175").unwrap();
    let script_pubkey = Script::from_bytes(&data);

    let input_index = 1;
    let input_amount = 3000000u64;

    let input = &tx.input[input_index];
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    let mut checker = TransactionSignatureChecker::new(&tx, input_index, input_amount);

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
fn test_arithemetic_correct_arguments_order() {
    let _ = sc_tracing::logging::LoggerBuilder::new("subcoin_script=debug").init();

    // https://www.blockchain.com/explorer/transactions/btc/54fabd73f1d20c980a0686bf0035078e07f69c58437e4d586fb29aa0bee9814f
    let tx = "01000000010c0e314bd7bb14721b3cfd8e487cd6866173354f87ca2cf4d13c8d3feb4301a6000000004a483045022100d92e4b61452d91a473a43cde4b469a472467c0ba0cbd5ebba0834e4f4762810402204802b76b7783db57ac1f61d2992799810e173e91055938750815b6d8a675902e014fffffffff0140548900000000001976a914a86e8ee2a05a44613904e18132e49b2448adc4e688ac00000000";
    let tx = decode_raw_tx(tx);

    let pkscript = hex::decode("76009f69905160a56b210378d430274f8c5ec1321338151e9f27f4c676a008bdf8638d07c0b6be9ab35c71ad6c").unwrap();
    let script_pubkey = Script::from_bytes(&pkscript);

    let input_index = 0;
    let input_amount = 0u64;

    let input = &tx.input[input_index];
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    let mut checker = TransactionSignatureChecker::new(&tx, input_index, input_amount);

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

#[test]
fn test_opreturn() {
    let _ = sc_tracing::logging::LoggerBuilder::new("subcoin_script=debug").init();

    // https://www.blockchain.com/explorer/transactions/btc/da47bd83967d81f3cf6520f4ff81b3b6c4797bfe7ac2b5969aedbf01a840cda6
    let tx = "0100000003fe1a25c8774c1e827f9ebdae731fe609ff159d6f7c15094e1d467a99a01e03100000000002012affffffff53a080075d834402e916390940782236b29d23db6f52dfc940a12b3eff99159c0000000000ffffffff61e4ed95239756bbb98d11dcf973146be0c17cc1cc94340deb8bc4d44cd88e92000000000a516352676a675168948cffffffff0220aa4400000000001976a9149bc0bbdd3024da4d0c38ed1aecf5c68dd1d3fa1288ac20aa4400000000001976a914169ff4804fd6596deb974f360c21584aa1e19c9788ac00000000";
    let tx = decode_raw_tx(tx);

    let pkscript = hex::decode("63516751676a68").unwrap();
    let script_pubkey = Script::from_bytes(&pkscript);

    let input_index = 2;
    let input_amount = 9000000;

    let input = &tx.input[input_index];
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    let mut checker = TransactionSignatureChecker::new(&tx, input_index, input_amount);

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
