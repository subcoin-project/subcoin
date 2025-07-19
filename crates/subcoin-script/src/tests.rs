mod opcode;
mod witness;

use crate::num::NumError;
use crate::signature_checker::SECP;
use crate::{EcdsaSignature, Error, TransactionSignatureChecker, VerifyFlags, verify_script};
use bitcoin::consensus::encode::deserialize_hex;
use bitcoin::secp256k1::Message;
use bitcoin::{PublicKey, Script, ScriptBuf, Transaction};

fn decode_pubkey(pubkey_hex: &str) -> PublicKey {
    PublicKey::from_slice(&hex::decode(pubkey_hex).unwrap()).unwrap()
}

fn test_verify_script(tx: &str, pkscript: &str, input_index: usize, input_amount: u64) {
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    let verify_result = Ok(());
    test_verify_script_full(
        tx,
        pkscript,
        input_index,
        input_amount,
        flags,
        verify_result,
    );
}

fn test_verify_script_with_flags(
    tx: &str,
    pkscript: &str,
    input_index: usize,
    input_amount: u64,
    flags: VerifyFlags,
) {
    let verify_result = Ok(());
    test_verify_script_full(
        tx,
        pkscript,
        input_index,
        input_amount,
        flags,
        verify_result,
    );
}

fn test_verify_script_full(
    tx: &str,
    pkscript: &str,
    input_index: usize,
    input_amount: u64,
    flags: VerifyFlags,
    verify_result: Result<(), Error>,
) {
    let _ = sc_tracing::logging::LoggerBuilder::new("subcoin_script=debug").init();

    let tx: Transaction = deserialize_hex(tx).unwrap();

    let pkscript = hex::decode(pkscript).unwrap();
    let script_pubkey = Script::from_bytes(&pkscript);

    let input = &tx.input[input_index];
    let mut checker = TransactionSignatureChecker::new(&tx, input_index, input_amount);

    tracing::debug!(
        "[test_verify_script_full] script_sig: {}",
        hex::encode(input.script_sig.as_bytes())
    );

    assert_eq!(
        verify_script(
            &input.script_sig,
            script_pubkey,
            &input.witness,
            &flags,
            &mut checker,
        ),
        verify_result
    );
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
    let tx: Transaction = deserialize_hex(tx).unwrap();

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
    // Block 110300:16:0
    // https://www.blockchain.com/explorer/transactions/btc/c99c49da4c38af669dea436d3e73780dfdb6c1ecf9958baa52960e8baee30e73
    let tx = "01000000010276b76b07f4935c70acf54fbf1f438a4c397a9fb7e633873c4dd3bc062b6b40000000008c493046022100d23459d03ed7e9511a47d13292d3430a04627de6235b6e51a40f9cd386f2abe3022100e7d25b080f0bb8d8d5f878bba7d54ad2fda650ea8d158a33ee3cbd11768191fd004104b0e2c879e4daf7b9ab68350228c159766676a14f5815084ba166432aab46198d4cca98fa3e9981d0a90b2effc514b76279476550ba3663fdcaff94c38420e9d5000000000100093d00000000001976a9149a7b0f3b80c6baaeedce0a0842553800f832ba1f88ac00000000";
    let pkscript = "76a914dc44b1164188067c3a32d4780f5996fa14a4f2d988ac";
    let input_index = 0;
    let input_amount = 5000000u64;

    test_verify_script(tx, pkscript, input_index, input_amount);
}

#[test]
fn test_signature_from_der_lax() {
    // Block 110300:16:0
    // https://www.blockchain.com/explorer/transactions/btc/c99c49da4c38af669dea436d3e73780dfdb6c1ecf9958baa52960e8baee30e73
    let tx = "01000000012316aac445c13ff31af5f3d1e2cebcada83e54ba10d15e01f49ec28bddc285aa000000008e4b3048022200002b83d59c1d23c08efd82ee0662fec23309c3adbcbd1f0b8695378db4b14e736602220000334a96676e58b1bb01784cb7c556dd8ce1c220171904da22e18fe1e7d1510db5014104d0fe07ff74c9ef5b00fed1104fad43ecf72dbab9e60733e4f56eacf24b20cf3b8cd945bcabcc73ba0158bf9ce769d43e94bd58c5c7e331a188922b3fe9ca1f5affffffff01c0c62d00000000001976a9147a2a3b481ca80c4ba7939c54d9278e50189d94f988ac00000000";
    let pkscript = "76a9147a2a3b481ca80c4ba7939c54d9278e50189d94f988ac";
    let input_index = 0;
    let input_amount = 3000000u64;

    test_verify_script(tx, pkscript, input_index, input_amount);

    // Block 140493:82:0
    // https://www.blockchain.com/explorer/transactions/btc/70f7c15c6f62139cc41afa858894650344eda9975b46656d893ee59df8914a3d
    let tx = "0100000001289eb02e8ddc1ee3486aadc1cd1335fba22a8e3e87e3f41b7c5bbe7fb4391d81010000008a47304402206b5c3b1c86748dcf328b9f3a65e10085afcf5d1af5b40970d8ce3a9355e06b5b0220cdbdc23e6d3618e47056fccc60c5f73d1a542186705197e5791e97f0e6582a32014104f25ec495fa21ad14d69f45bf277129488cfb1a339aba1fed3c5099bb6d8e9716491a14050fbc0b2fed2963dc1e56264b3adf52a81b953222a2180d48b54d1e18ffffffff0140420f00000000001976a914e6ba8cc407375ce1623ec17b2f1a59f2503afc6788ac00000000";
    let pkscript = "76a914c62301ef02dfeec757fb8dedb8a45eda5fb5ee4d88ac";
    let input_index = 0;
    let input_amount = 1000000u64;

    test_verify_script(tx, pkscript, input_index, input_amount);
}

#[test]
fn test_multisig_may_contain_invalid_signature() {
    // https://www.blockchain.com/explorer/transactions/btc/bc179baab547b7d7c1d5d8d6f8b0cc6318eaa4b0dd0a093ad6ac7f5a1cb6b3ba
    let tx = "010000000290c5e425bfba62bd5b294af0414d8fa3ed580c5ca6f351ccc23e360b14ff7f470100000091004730440220739d9ab2c3e7089e7bd311f267a65dc0ea00f49619cb61ec016a5038016ed71202201b88257809b623d471e429787c36e0a9bcd2a058fc0c75fd9c25f905657e3b9e01ab512103c86390eb5230237f31de1f02e70ce61e77f6dbfefa7d0e4ed4f6b3f78f85d8ec2103193f28067b502b34cac9eae39f74dba4815e1278bab31516efb29bd8de2c1bea52aeffffffffdd7f3ce640a2fb04dbe24630aa06e4299fbb1d3fe585fe4f80be4a96b5ff0a0d01000000b400483045022100a28d2ace2f1cb4b2a58d26a5f1a2cc15cdd4cf1c65cee8e4521971c7dc60021c0220476a5ad62bfa7c18f9174d9e5e29bc0062df543e2c336ae2c77507e462bbf95701ab512103c86390eb5230237f31de1f02e70ce61e77f6dbfefa7d0e4ed4f6b3f78f85d8ec2103193f28067b502b34cac9eae39f74dba4815e1278bab31516efb29bd8de2c1bea21032462c60ebc21f4d38b3c4ccb33be77b57ae72762be12887252db18fd6225befb53aeffffffff02e0fd1c00000000001976a9148501106ab5492387998252403d70857acfa1586488ac50c3000000000000171499050637f553f03cc0f82bbfe98dc99f10526311b17500000000";
    let pkscript = "1464d63d835705618da2111ca3194f22d067187cf2b175";
    let input_index = 0;
    let input_amount = 1000000u64;

    test_verify_script(tx, pkscript, input_index, input_amount);
}

#[test]
fn test_multisig_may_contain_invalid_pubkey() {
    // https://www.blockchain.com/explorer/transactions/btc/70c4e749f2b8b907875d1483ae43e8a6790b0c8397bbb33682e3602617f9a77a
    let tx = "01000000025718fb915fb8b3a802bb699ddf04dd91261ef6715f5f2820a2b1b9b7e38b4f27000000004a004830450221008c2107ed4e026ab4319a591e8d9ec37719cdea053951c660566e3a3399428af502202ecd823d5f74a77cc2159d8af2d3ea5d36a702fef9a7edaaf562aef22ac35da401ffffffff038f52231b994efb980382e4d804efeadaee13cfe01abe0d969038ccb45ec17000000000490047304402200487cd787fde9b337ab87f9fe54b9fd46d5d1692aa58e97147a4fe757f6f944202203cbcfb9c0fc4e3c453938bbea9e5ae64030cf7a97fafaf460ea2cb54ed5651b501ffffffff0100093d00000000001976a9144dc39248253538b93d3a0eb122d16882b998145888ac00000000";
    let pkscript = "51210351efb6e91a31221652105d032a2508275f374cea63939ad72f1b1e02f477da782100f2b7816db49d55d24df7bdffdbc1e203b424e8cd39f5651ab938e5e4a193569e52ae";
    let input_index = 0;
    let input_amount = 2u64;

    test_verify_script(tx, pkscript, input_index, input_amount);
}

#[test]
fn test_check_transaction_multisig() {
    // https://www.blockchain.com/explorer/transactions/btc/02b082113e35d5386285094c2829e7e2963fa0b5369fb7f4b79c4c90877dcd3d
    let tx = "01000000013dcd7d87904c9cb7f4b79f36b5a03f96e2e729284c09856238d5353e1182b00200000000fd5e0100483045022100deeb1f13b5927b5e32d877f3c42a4b028e2e0ce5010fdb4e7f7b5e2921c1dcd2022068631cb285e8c1be9f061d2968a18c3163b780656f30a049effee640e80d9bff01483045022100ee80e164622c64507d243bd949217d666d8b16486e153ac6a1f8e04c351b71a502203691bef46236ca2b4f5e60a82a853a33d6712d6a1e7bf9a65e575aeb7328db8c014cc9524104a882d414e478039cd5b52a92ffb13dd5e6bd4515497439dffd691a0f12af9575fa349b5694ed3155b136f09e63975a1700c9f4d4df849323dac06cf3bd6458cd41046ce31db9bdd543e72fe3039a1f1c047dab87037c36a669ff90e28da1848f640de68c2fe913d363a51154a0c62d7adea1b822d05035077418267b1a1379790187410411ffd36c70776538d079fbae117dc38effafb33304af83ce4894589747aee1ef992f63280567f52f5ba870678b4ab4ff6c8ea600bd217870a8b4f1f09f3a8e8353aeffffffff0130d90000000000001976a914569076ba39fc4ff6a2291d9ea9196d8c08f9c7ab88ac00000000";
    let pkscript = "a9141a8b0026343166625c7475f01e48b5ede8c0252e87";
    let input_index = 0;
    let input_amount = 75600u64;

    test_verify_script(tx, pkscript, input_index, input_amount);
}

#[test]
fn test_transaction_with_high_s_signature() {
    // https://www.blockchain.com/explorer/transactions/btc/12b5633bad1f9c167d523ad1aa1947b2732a865bf5414eab2f9e5ae5d5c191ba?show_adv=true
    let tx = "010000000173805864da01f15093f7837607ab8be7c3705e29a9d4a12c9116d709f8911e590100000049483045022052ffc1929a2d8bd365c6a2a4e3421711b4b1e1b8781698ca9075807b4227abcb0221009984107ddb9e3813782b095d0d84361ed4c76e5edaf6561d252ae162c2341cfb01ffffffff0200e1f50500000000434104baa9d36653155627c740b3409a734d4eaf5dcca9fb4f736622ee18efcf0aec2b758b2ec40db18fbae708f691edb2d4a2a3775eb413d16e2e3c0f8d4c69119fd1ac009ce4a60000000043410411db93e1dcdb8a016b49840f8c53bc1eb68a382e97b1482ecad7b148a6909a5cb2e0eaddfb84ccf9744464f82e160bfa9b8b64f9d4c03f999b8643f656b412a3ac00000000";
    let pkscript = "410411db93e1dcdb8a016b49840f8c53bc1eb68a382e97b1482ecad7b148a6909a5cb2e0eaddfb84ccf9744464f82e160bfa9b8b64f9d4c03f999b8643f656b412a3ac";
    let input_index = 0;
    let input_amount = 2900000000u64;

    test_verify_script(tx, pkscript, input_index, input_amount);
}

#[test]
fn test_transaction_from_124276() {
    // https://www.blockchain.com/explorer/transactions/btc/fb0a1d8d34fa5537e461ac384bac761125e1bfa7fec286fa72511240fa66864d
    let tx = "01000000012316aac445c13ff31af5f3d1e2cebcada83e54ba10d15e01f49ec28bddc285aa000000008e4b3048022200002b83d59c1d23c08efd82ee0662fec23309c3adbcbd1f0b8695378db4b14e736602220000334a96676e58b1bb01784cb7c556dd8ce1c220171904da22e18fe1e7d1510db5014104d0fe07ff74c9ef5b00fed1104fad43ecf72dbab9e60733e4f56eacf24b20cf3b8cd945bcabcc73ba0158bf9ce769d43e94bd58c5c7e331a188922b3fe9ca1f5affffffff01c0c62d00000000001976a9147a2a3b481ca80c4ba7939c54d9278e50189d94f988ac00000000";
    let pkscript = "76a9147a2a3b481ca80c4ba7939c54d9278e50189d94f988ac";
    let input_index = 0;
    let input_amount = 0u64;

    test_verify_script(tx, pkscript, input_index, input_amount);
}

#[test]
fn test_transaction_bip65() {
    // https://www.blockchain.com/explorer/transactions/btc/eb3b82c0884e3efa6d8b0be55b4915eb20be124c9766245bcc7f34fdac32bccb
    let tx = "01000000024de8b0c4c2582db95fa6b3567a989b664484c7ad6672c85a3da413773e63fdb8000000006b48304502205b282fbc9b064f3bc823a23edcc0048cbb174754e7aa742e3c9f483ebe02911c022100e4b0b3a117d36cab5a67404dddbf43db7bea3c1530e0fe128ebc15621bd69a3b0121035aa98d5f77cd9a2d88710e6fc66212aff820026f0dad8f32d1f7ce87457dde50ffffffff4de8b0c4c2582db95fa6b3567a989b664484c7ad6672c85a3da413773e63fdb8010000006f004730440220276d6dad3defa37b5f81add3992d510d2f44a317fd85e04f93a1e2daea64660202200f862a0da684249322ceb8ed842fb8c859c0cb94c81e1c5308b4868157a428ee01ab51210232abdc893e7f0631364d7fd01cb33d24da45329a00357b3a7886211ab414d55a51aeffffffff02e0fd1c00000000001976a914380cb3c594de4e7e9b8e18db182987bebb5a4f7088acc0c62d000000000017142a9bc5447d664c1d0141392a842d23dba45c4f13b17500000000";
    let pkscript = "142a9bc5447d664c1d0141392a842d23dba45c4f13b175";
    let input_index = 1;
    let input_amount = 3000000u64;

    test_verify_script(tx, pkscript, input_index, input_amount);

    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS | VerifyFlags::CHECKLOCKTIMEVERIFY;

    test_verify_script_full(
        tx,
        pkscript,
        input_index,
        input_amount,
        flags,
        Err(Error::Num(NumError::Overflow)),
    );
}

#[test]
fn test_arithemetic_correct_arguments_order() {
    // https://www.blockchain.com/explorer/transactions/btc/54fabd73f1d20c980a0686bf0035078e07f69c58437e4d586fb29aa0bee9814f
    let tx = "01000000010c0e314bd7bb14721b3cfd8e487cd6866173354f87ca2cf4d13c8d3feb4301a6000000004a483045022100d92e4b61452d91a473a43cde4b469a472467c0ba0cbd5ebba0834e4f4762810402204802b76b7783db57ac1f61d2992799810e173e91055938750815b6d8a675902e014fffffffff0140548900000000001976a914a86e8ee2a05a44613904e18132e49b2448adc4e688ac00000000";
    let pkscript = "76009f69905160a56b210378d430274f8c5ec1321338151e9f27f4c676a008bdf8638d07c0b6be9ab35c71ad6c";
    let input_index = 0;
    let input_amount = 0u64;

    test_verify_script(tx, pkscript, input_index, input_amount);
}

#[test]
fn test_opreturn_is_handled_properly() {
    // https://www.blockchain.com/explorer/transactions/btc/da47bd83967d81f3cf6520f4ff81b3b6c4797bfe7ac2b5969aedbf01a840cda6
    let tx = "0100000003fe1a25c8774c1e827f9ebdae731fe609ff159d6f7c15094e1d467a99a01e03100000000002012affffffff53a080075d834402e916390940782236b29d23db6f52dfc940a12b3eff99159c0000000000ffffffff61e4ed95239756bbb98d11dcf973146be0c17cc1cc94340deb8bc4d44cd88e92000000000a516352676a675168948cffffffff0220aa4400000000001976a9149bc0bbdd3024da4d0c38ed1aecf5c68dd1d3fa1288ac20aa4400000000001976a914169ff4804fd6596deb974f360c21584aa1e19c9788ac00000000";
    let pkscript = "63516751676a68";
    let input_index = 2;
    let input_amount = 9000000;

    test_verify_script(tx, pkscript, input_index, input_amount);
}

#[test]
fn test_sigscript_may_contain_empty_signature() {
    // https://www.blockchain.com/explorer/transactions/btc/9fb65b7304aaa77ac9580823c2c06b259cc42591e5cce66d76a81b6f51cc5c28
    let tx = "0100000001545959fea5f64c7d0cc4199a175ebfa6eb29415cf72acd7676dedaad8170cea6000000008c20ca42095840735e89283fec298e62ac2ddea9b5f34a8cbb7097ad965b87568100201b1b01dc829177da4a14551d2fc96a9db00c6501edfa12f22cd9cefd335c227f483045022100940a7a74d00d590dc6743c8d7416475845611047bed013db2c4f80c96261576a02202b223572a37c2eee65fefabf701b2f39e3df2cba5d136eef0485d9a24f49c0aa0100ffffffff010071020000000000232103611f9a45c18f28f06f19076ad571c344c82ce8fcfe34464cf8085217a2d294a6ac00000000";
    let pkscript = "2102085c6600657566acc2d6382a47bc3f324008d2aa10940dd7705a48aa2a5a5e33ac7c2103f5d0fb955f95dd6be6115ce85661db412ec6a08abcbfce7da0ba8297c6cc0ec4ac7c5379a820d68df9e32a147cffa36193c6f7c43a1c8c69cda530e1c6db354bfabdcfefaf3c875379a820f531f3041d3136701ea09067c53e7159c8f9b2746a56c3d82966c54bbc553226879a5479827701200122a59a5379827701200122a59a6353798277537982778779679a68";
    let input_index = 0;
    let input_amount = 170000;

    test_verify_script(tx, pkscript, input_index, input_amount);
}

#[test]
fn multisig_may_contain_invalid_signature_that_can_not_be_parsed_by_from_der_lax() {
    // https://www.blockchain.com/explorer/transactions/btc/fd9b541d23f6e9bddb34ede15c7684eeec36231118796b691ae525f95578acf1
    let tx = "01000000011f60b3d94284e3c321eb8290c2d883812be5d3d3fb9b0611fd6c04df80e254350000000008515151515102ae91ffffffff01a0bb0d000000000017a914827fe37ec405346ad4e995323cea83559537b89e8700000000";
    let pkscript = "a91438430c4d1c214bf11d2c0c3dea8e5e9a5d11aab087";
    let input_index = 0;
    let input_amount = 1000000;

    test_verify_script(tx, pkscript, input_index, input_amount);
}

#[test]
fn test_cltv_should_work() {
    // https://www.blockchain.com/explorer/transactions/btc/7eac45ade99281c1458aca234f2ea260fee9075065d260b0d3118df8bd2fedf3
    let tx = "010000000168317ec8ecaee38c03a21939dde22622b37c0fb6d135fa3c3f9d2da71374ab3d0100000073483045022100b2e827d892be74e00a7d499244e6fc2ec392ba2c6dc7c72bbd99f31865d346450220612182f6bb99b7aec87e82945243b86f327411c3d1f3a6a2abf6bd1317c584af0129034ded05b17521038cd6db60cb937555dcd30d6f927a3e4ee0f631e8eaf1c61c76c40804ffa3edd4ac000000000194bb00000000000017a914f1f4fb78c04fe29032192919b3d90c6a5ccaddab874ded0500";
    let pkscript = "a914f1f4fb78c04fe29032192919b3d90c6a5ccaddab87";
    let input_index = 0;
    let input_amount = 50000;

    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS | VerifyFlags::CHECKLOCKTIMEVERIFY;
    test_verify_script_with_flags(tx, pkscript, input_index, input_amount, flags);
}

#[test]
fn test_code_separators_should_be_removed_for_base_signature_verification() {
    // https://www.blockchain.com/explorer/transactions/btc/7eac45ade99281c1458aca234f2ea260fee9075065d260b0d3118df8bd2fedf3
    let tx = "01000000016aaa18f4ab91fab80ecda666c4def68b8b75cc6bb1169ecd81716eab03ff14d007000000fd8701483045022100ac4319cf798ab10d864ad5f206cd405b7a15957eef2b0094ab24ffcf2c28fbfb022012053c8142d9e4f832d85c6ce7dba82d44d011c7713fb584771fb8770da97c0c012102c8662aaa171b5c98fef66c02138165f600c7c5743380686958e395edf8eb36bf47304402202feedc3b54cd87868406e93ee650742b61ce39162d70b6fde5a805fd40a56c900220015970a2fc874c32edfcd6341981d35e5b019a14b17662e00f49e363db72b93c014cd22102fb6827937707bf432d85b094bc180ab93394ee013b3ecaafa04b9135e3ab6e50ad74926404162c5658b15167762103db22e387923ad0552e1c4a4355324313af85926d4266c0eaa86f02eb1e01b2d28763ac67762102c8662aaa171b5c98fef66c02138165f600c7c5743380686958e395edf8eb36bf886e6b6b0064ab05636f6e643175ac687664756c6c6e6b6bab05636f6e643275ac687664756c6c6e6b6bab05636f6e643375ac687664756c6c6e6b6bab05636f6e643475ac687664756c6c6e6b6bab05636f6e643575ac686868ffffffff01204e0000000000001976a914648a4310b84426f426398ef27e3388a4d2c05a2888ac342c5658";
    let pkscript = "a9143ae52dbc43c884ef43211a43082d01a0091ef1e387";
    let input_index = 0;
    let input_amount = 70000;

    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    test_verify_script_with_flags(tx, pkscript, input_index, input_amount, flags);
}

// https://github.com/rust-bitcoin/rust-bitcoin/issues/4112
#[test]
fn test_sighash_single_bug_should_be_handled_properly() {
    // https://www.blockchain.com/explorer/transactions/btc/1395dde8ec0152f4cb090f4d4e8dcb14cc0d8bbba780e686c44934370cd316ec
    let tx = "0100000005238e0fd85f30e2072141d80e3f8835fdf2475cc94264804b1e18c3d47b7125ec020000006a473044022042cb0081018fe929f2200588dd78dd894e8971d4c7d78d1b94687ac5f9fa728902203aaa065472d4c1e8e5e596b877356f237f956d4c3215ed801c0db85d9abf0cc5832102539172db3f6f4f6f97f21aee74f1e3ad7afc7a8c378f823d8aa12a48a04d88e9ffffffff484721321d2f1eaefb80861748f239269bc3b3aeaa7933d287792778e6ac43fa000000006b483045022100a33646af02aea9f627d6109d816dea4d91b6cb2b049d3ab9d23d3fd2530b4eb902207d64f947baede3f76d1fe17157d2cb77c924c176e67a6134f43893fabe1657f8832102539172db3f6f4f6f97f21aee74f1e3ad7afc7a8c378f823d8aa12a48a04d88e9ffffffffaf93fa5e2fdcb4f2ab0b6a5ee952d08c6c50ee3b9d0a93c618f72e870f9581c4000000006a473044022063d96da5131fdcfcad5f5e3eb2c4e2f747f4da56252f21e4db9e9120483be6f2022031d4396eda80dcca907a414cba9e9415ed6666ab99d27cc51a08711f6fd8cc2c832102539172db3f6f4f6f97f21aee74f1e3ad7afc7a8c378f823d8aa12a48a04d88e9ffffffffacba187db41514c706bc4158d89edc1cf09ea943bb129eaf14d8e938dcc761a4000000006a473044022063d96da5131fdcfcad5f5e3eb2c4e2f747f4da56252f21e4db9e9120483be6f2022031d4396eda80dcca907a414cba9e9415ed6666ab99d27cc51a08711f6fd8cc2c832102539172db3f6f4f6f97f21aee74f1e3ad7afc7a8c378f823d8aa12a48a04d88e9ffffffff6098091cf010883998204336aeebc43f04628230c769add0145f4e48656922bd000000006a473044022063d96da5131fdcfcad5f5e3eb2c4e2f747f4da56252f21e4db9e9120483be6f2022031d4396eda80dcca907a414cba9e9415ed6666ab99d27cc51a08711f6fd8cc2c832102539172db3f6f4f6f97f21aee74f1e3ad7afc7a8c378f823d8aa12a48a04d88e9ffffffff02fac53b00000000001976a914f1e7b93fde46772793af5c06a8e4120cb8521de988ac00000000000000000a6a08456c657068656e7400000000";
    let pkscript = "76a9147fb1c3c81973a51d2dc978fe70ac83d869802ba188ac";
    let input_index = 2;
    let input_amount = 90000;

    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    test_verify_script_with_flags(tx, pkscript, input_index, input_amount, flags);
}

// https://github.com/rust-bitcoin/rust-bitcoin/issues/4133
#[test]
fn test_non_standard_sighash_type_should_be_encoded_correctly() {
    // https://www.blockchain.com/explorer/transactions/btc/1395dde8ec0152f4cb090f4d4e8dcb14cc0d8bbba780e686c44934370cd316ec
    let tx = "01000000000101447e208868dbc8e930fc6eba4fe0d0abfe0d9dc2db4ba70542e02467f00205c90100000017160014e20c60563894174c253ae937ba59ace46ab9ffb1ffffffff010845f305000000001976a91414ac7fc2a782bde1555b753d75ff4ed146683cae88ac024730440220120003c32cca7eabf07bad5c31125accc09d13c39546fa93833b8b69a2c72ed7022057083dc2ed348156874b8af859ac7a9c16e5ce39353f3f1ac2226b49c2b319af652103f73386ac6e567581f8d0611ad7a8536c3cd0253e535f6fc4707514b2ab54198700000000";
    let pkscript = "a914e93f9e95f6d5cb1736a94de992d0d18819072fa587";
    let input_index = 0;
    let input_amount = 99830000;

    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    test_verify_script_with_flags(tx, pkscript, input_index, input_amount, flags);
}
