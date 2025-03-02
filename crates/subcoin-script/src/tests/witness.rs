//! Witness related script tests ported from https://github.com/paritytech/parity-bitcoin/blob/master/script/src/interpreter.rs#L2416-L3245

use crate::{verify_script, Error, TransactionSignatureChecker, VerifyFlags};
use bitcoin::consensus::encode::deserialize_hex;
use bitcoin::hashes::Hash;
use bitcoin::script::{Builder, Script};
use bitcoin::transaction::Version;
use bitcoin::{Amount, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid, Witness};

fn run_witness_test(
    script_sig: &str,
    script_pubkey: &str,
    script_witness: Vec<&str>,
    flags: VerifyFlags,
    amount: u64,
) -> Result<(), Error> {
    let sig = hex::decode(script_sig).unwrap();
    let script_sig = Script::from_bytes(&sig);
    let pk = hex::decode(script_pubkey).unwrap();
    let script_pubkey = Script::from_bytes(&pk);
    let witness: Witness = script_witness
        .iter()
        .map(|x| hex::decode(x).unwrap())
        .collect::<Vec<_>>()
        .into();

    let tx1 = Transaction {
        version: Version::ONE,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint {
                txid: Txid::all_zeros(),
                vout: 0xffffffff,
            },
            script_sig: Builder::new().push_int(0).push_int(0).into_script(),
            sequence: Sequence::MAX,
            witness: Witness::default(),
        }],
        output: vec![TxOut {
            value: Amount::from_sat(amount),
            script_pubkey: script_pubkey.to_owned(),
        }],
    };

    let tx2 = Transaction {
        version: Version::ONE,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint {
                txid: tx1.compute_txid(),
                vout: 0,
            },
            script_sig: script_sig.to_owned(),
            sequence: Sequence::MAX,
            witness: witness.clone(),
        }],
        output: vec![TxOut {
            value: Amount::from_sat(amount),
            script_pubkey: ScriptBuf::new(),
        }],
    };

    let input_index = 0;

    let mut checker = TransactionSignatureChecker::new(&tx2, input_index, amount);

    verify_script(script_sig, script_pubkey, &witness, &flags, &mut checker)
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/script_tests.json#L1257
#[test]
fn witness_invalid_script() {
    assert_eq!(
        Err(Error::EvalFalse),
        run_witness_test(
            "",
            "00206e340b9cffb37a989ca544e6bb780a2c78901d3fb33738768511a30617afa01d",
            vec!["00"],
            VerifyFlags::P2SH | VerifyFlags::WITNESS,
            0,
        )
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/script_tests.json#L1258
#[test]
fn witness_script_hash_mismatch() {
    assert_eq!(
        Err(Error::WitnessProgramMismatch),
        run_witness_test(
            "",
            "00206e340b9cffb37a989ca544e6bb780a2c78901d3fb33738768511a30617afa01d",
            vec!["51"],
            VerifyFlags::P2SH | VerifyFlags::WITNESS,
            0,
        )
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/script_tests.json#L1259
#[test]
fn witness_invalid_script_check_skipped() {
    assert_eq!(
        Ok(()),
        run_witness_test(
            "",
            "00206e340b9cffb37a989ca544e6bb780a2c78901d3fb33738768511a30617afa01d",
            vec!["00"],
            VerifyFlags::NONE,
            0,
        )
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/script_tests.json#L1260
#[test]
fn witness_script_hash_mismatch_check_skipped() {
    assert_eq!(
        Ok(()),
        run_witness_test(
            "",
            "00206e340b9cffb37a989ca544e6bb780a2c78901d3fb33738768511a30617afa01d",
            vec!["51"],
            VerifyFlags::NONE,
            0,
        )
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/script_tests.json#L1860
#[test]
fn witness_basic_p2wsh() {
    assert_eq!(
        run_witness_test(
            "",
            "0020b95237b48faaa69eb078e1170be3b5cbb3fddf16d0a991e14ad274f7b33a4f64",
            vec![
                "304402200d461c140cfdfcf36b94961db57ae8c18d1cb80e9d95a9e47ac22470c1bf125502201c8dc1cbfef6a3ef90acbbb992ca22fe9466ee6f9d4898eda277a7ac3ab4b25101",
                "410479be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798483ada7726a3c4655da4fbfc0e1108a8fd17b448a68554199c47d08ffb10d4b8ac",
            ],
            VerifyFlags::P2SH | VerifyFlags::WITNESS,
            1,
        ),
        Ok(())
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/script_tests.json#L1872
#[test]
fn witness_basic_p2wpkh() {
    assert_eq!(
        run_witness_test(
            "",
            "001491b24bf9f5288532960ac687abb035127b1d28a5",
            vec![
                "304402201e7216e5ccb3b61d46946ec6cc7e8c4e0117d13ac2fd4b152197e4805191c74202203e9903e33e84d9ee1dd13fb057afb7ccfb47006c23f6a067185efbc9dd780fc501",
                "0479be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798483ada7726a3c4655da4fbfc0e1108a8fd17b448a68554199c47d08ffb10d4b8",
            ],
            VerifyFlags::P2SH | VerifyFlags::WITNESS,
            1,
        ),
        Ok(())
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/script_tests.json#L1884
#[test]
fn witness_basic_p2sh_p2wsh() {
    assert_eq!(
        run_witness_test(
            "220020b95237b48faaa69eb078e1170be3b5cbb3fddf16d0a991e14ad274f7b33a4f64",
            "a914f386c2ba255cc56d20cfa6ea8b062f8b5994551887",
            vec![
                "3044022066e02c19a513049d49349cf5311a1b012b7c4fae023795a18ab1d91c23496c22022025e216342c8e07ce8ef51e8daee88f84306a9de66236cab230bb63067ded1ad301",
                "410479be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798483ada7726a3c4655da4fbfc0e1108a8fd17b448a68554199c47d08ffb10d4b8ac",
            ],
            VerifyFlags::P2SH | VerifyFlags::WITNESS,
            1,
        ),
        Ok(())
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/script_tests.json#L1896
#[test]
fn witness_basic_p2sh_p2wpkh() {
    assert_eq!(
        run_witness_test(
            "16001491b24bf9f5288532960ac687abb035127b1d28a5",
            "a91417743beb429c55c942d2ec703b98c4d57c2df5c687",
            vec![
                "304402200929d11561cd958460371200f82e9cae64c727a495715a31828e27a7ad57b36d0220361732ced04a6f97351ecca21a56d0b8cd4932c1da1f8f569a2b68e5e48aed7801",
                "0479be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798483ada7726a3c4655da4fbfc0e1108a8fd17b448a68554199c47d08ffb10d4b8",
            ],
            VerifyFlags::P2SH | VerifyFlags::WITNESS,
            1,
        ),
        Ok(())
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/script_tests.json#L1908
#[test]
fn witness_basic_p2wsh_with_wrong_key() {
    assert_eq!(
        run_witness_test(
            "",
            "0020ac8ebd9e52c17619a381fa4f71aebb696087c6ef17c960fd0587addad99c0610",
            vec![
                "304402202589f0512cb2408fb08ed9bd24f85eb3059744d9e4f2262d0b7f1338cff6e8b902206c0978f449693e0578c71bc543b11079fd0baae700ee5e9a6bee94db490af9fc01",
                "41048282263212c609d9ea2a6e3e172de238d8c39cabd5ac1ca10646e23fd5f5150811f8a8098557dfe45e8256e830b60ace62d613ac2f7b17bed31b6eaff6e26cafac",
            ],
            VerifyFlags::P2SH | VerifyFlags::WITNESS,
            0,
        ),
        Err(Error::EvalFalse),
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/script_tests.json#L1920
#[test]
fn witness_basic_p2wpkh_with_wrong_key() {
    assert_eq!(
        run_witness_test(
            "",
            "00147cf9c846cd4882efec4bf07e44ebdad495c94f4b",
            vec![
                "304402206ef7fdb2986325d37c6eb1a8bb24aeb46dede112ed8fc76c7d7500b9b83c0d3d02201edc2322c794fe2d6b0bd73ed319e714aa9b86d8891961530d5c9b7156b60d4e01",
                "048282263212c609d9ea2a6e3e172de238d8c39cabd5ac1ca10646e23fd5f5150811f8a8098557dfe45e8256e830b60ace62d613ac2f7b17bed31b6eaff6e26caf",
            ],
            VerifyFlags::P2SH | VerifyFlags::WITNESS,
            0,
        ),
        Err(Error::EvalFalse),
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/script_tests.json#L1920
#[test]
fn witness_basic_p2sh_p2wsh_with_wrong_key() {
    assert_eq!(
        run_witness_test(
            "220020ac8ebd9e52c17619a381fa4f71aebb696087c6ef17c960fd0587addad99c0610",
            "a91461039a003883787c0d6ebc66d97fdabe8e31449d87",
            vec![
                "30440220069ea3581afaf8187f63feee1fd2bd1f9c0dc71ea7d6e8a8b07ee2ebcf824bf402201a4fdef4c532eae59223be1eda6a397fc835142d4ddc6c74f4aa85b766a5c16f01",
                "41048282263212c609d9ea2a6e3e172de238d8c39cabd5ac1ca10646e23fd5f5150811f8a8098557dfe45e8256e830b60ace62d613ac2f7b17bed31b6eaff6e26cafac",
            ],
            VerifyFlags::P2SH | VerifyFlags::WITNESS,
            0,
        ),
        Err(Error::EvalFalse),
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/script_tests.json#L1944
#[test]
fn witness_basic_p2sh_p2wpkh_with_wrong_key() {
    assert_eq!(
        run_witness_test(
            "1600147cf9c846cd4882efec4bf07e44ebdad495c94f4b",
            "a9144e0c2aed91315303fc6a1dc4c7bc21c88f75402e87",
            vec![
                "304402204209e49457c2358f80d0256bc24535b8754c14d08840fc4be762d6f5a0aed80b02202eaf7d8fc8d62f60c67adcd99295528d0e491ae93c195cec5a67e7a09532a88001",
                "048282263212c609d9ea2a6e3e172de238d8c39cabd5ac1ca10646e23fd5f5150811f8a8098557dfe45e8256e830b60ace62d613ac2f7b17bed31b6eaff6e26caf",
            ],
            VerifyFlags::P2SH | VerifyFlags::WITNESS,
            0,
        ),
        Err(Error::EvalFalse),
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/script_tests.json#L1956
#[test]
fn witness_basic_p2wsh_with_wrong_key_check_skipped() {
    assert_eq!(
        run_witness_test(
            "",
            "0020ac8ebd9e52c17619a381fa4f71aebb696087c6ef17c960fd0587addad99c0610",
            vec![
                "304402202589f0512cb2408fb08ed9bd24f85eb3059744d9e4f2262d0b7f1338cff6e8b902206c0978f449693e0578c71bc543b11079fd0baae700ee5e9a6bee94db490af9fc01",
                "41048282263212c609d9ea2a6e3e172de238d8c39cabd5ac1ca10646e23fd5f5150811f8a8098557dfe45e8256e830b60ace62d613ac2f7b17bed31b6eaff6e26cafac",
            ],
            VerifyFlags::P2SH,
            0,
        ),
        Ok(()),
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/script_tests.json#L1968
#[test]
fn witness_basic_p2wpkh_with_wrong_key_check_skipped() {
    assert_eq!(
        run_witness_test(
            "",
            "00147cf9c846cd4882efec4bf07e44ebdad495c94f4b",
            vec![
                "304402206ef7fdb2986325d37c6eb1a8bb24aeb46dede112ed8fc76c7d7500b9b83c0d3d02201edc2322c794fe2d6b0bd73ed319e714aa9b86d8891961530d5c9b7156b60d4e01",
                "4104828048282263212c609d9ea2a6e3e172de238d8c39cabd5ac1ca10646e23fd5f5150811f8a8098557dfe45e8256e830b60ace62d613ac2f7b17bed31b6eaff6e26caf2263212c609d9ea2a6e3e172de238d8c39cabd5ac1ca10646e23fd5f5150811f8a8098557dfe45e8256e830b60ace62d613ac2f7b17bed31b6eaff6e26cafac",
            ],
            VerifyFlags::P2SH,
            0,
        ),
        Ok(()),
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/script_tests.json#L1980
#[test]
fn witness_basic_p2sh_p2wsh_with_wrong_key_check_skipped() {
    assert_eq!(
        run_witness_test(
            "220020ac8ebd9e52c17619a381fa4f71aebb696087c6ef17c960fd0587addad99c0610",
            "a91461039a003883787c0d6ebc66d97fdabe8e31449d87",
            vec![
                "30440220069ea3581afaf8187f63feee1fd2bd1f9c0dc71ea7d6e8a8b07ee2ebcf824bf402201a4fdef4c532eae59223be1eda6a397fc835142d4ddc6c74f4aa85b766a5c16f01",
                "41048282263212c609d9ea2a6e3e172de238d8c39cabd5ac1ca10646e23fd5f5150811f8a8098557dfe45e8256e830b60ace62d613ac2f7b17bed31b6eaff6e26cafac",
            ],
            VerifyFlags::P2SH,
            0,
        ),
        Ok(()),
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/script_tests.json#L1992
#[test]
fn witness_basic_p2sh_p2wpkh_with_wrong_key_check_skipped() {
    assert_eq!(
        run_witness_test(
            "1600147cf9c846cd4882efec4bf07e44ebdad495c94f4b",
            "a9144e0c2aed91315303fc6a1dc4c7bc21c88f75402e87",
            vec![
                "304402204209e49457c2358f80d0256bc24535b8754c14d08840fc4be762d6f5a0aed80b02202eaf7d8fc8d62f60c67adcd99295528d0e491ae93c195cec5a67e7a09532a88001",
                "048282263212c609d9ea2a6e3e172de238d8c39cabd5ac1ca10646e23fd5f5150811f8a8098557dfe45e8256e830b60ace62d613ac2f7b17bed31b6eaff6e26caf",
            ],
            VerifyFlags::P2SH,
            0,
        ),
        Ok(()),
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/script_tests.json#L2004
#[test]
fn witness_basic_p2wsh_with_wrong_value() {
    assert_eq!(
        run_witness_test(
            "",
            "0020b95237b48faaa69eb078e1170be3b5cbb3fddf16d0a991e14ad274f7b33a4f64",
            vec![
                "3044022066faa86e74e8b30e82691b985b373de4f9e26dc144ec399c4f066aa59308e7c202204712b86f28c32503faa051dbeabff2c238ece861abc36c5e0b40b1139ca222f001",
                "410479be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798483ada7726a3c4655da4fbfc0e1108a8fd17b448a68554199c47d08ffb10d4b8ac",
            ],
            VerifyFlags::P2SH | VerifyFlags::WITNESS,
            0,
        ),
        Err(Error::EvalFalse),
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/script_tests.json#L2016
#[test]
fn witness_basic_p2wpkh_with_wrong_value() {
    assert_eq!(
        run_witness_test(
            "",
            "001491b24bf9f5288532960ac687abb035127b1d28a5",
            vec![
                "304402203b3389b87448d7dfdb5e82fb854fcf92d7925f9938ea5444e36abef02c3d6a9602202410bc3265049abb07fd2e252c65ab7034d95c9d5acccabe9fadbdc63a52712601",
                "0479be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798483ada7726a3c4655da4fbfc0e1108a8fd17b448a68554199c47d08ffb10d4b8",
            ],
            VerifyFlags::P2SH | VerifyFlags::WITNESS,
            0,
        ),
        Err(Error::EvalFalse),
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/script_tests.json#L2028
#[test]
fn witness_basic_p2sh_p2wsh_with_wrong_value() {
    assert_eq!(
        run_witness_test(
            "220020b95237b48faaa69eb078e1170be3b5cbb3fddf16d0a991e14ad274f7b33a4f64",
            "a914f386c2ba255cc56d20cfa6ea8b062f8b5994551887",
            vec![
                "3044022000a30c4cfc10e4387be528613575434826ad3c15587475e0df8ce3b1746aa210022008149265e4f8e9dafe1f3ea50d90cb425e9e40ea7ebdd383069a7cfa2b77004701",
                "410479be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798483ada7726a3c4655da4fbfc0e1108a8fd17b448a68554199c47d08ffb10d4b8ac",
            ],
            VerifyFlags::P2SH | VerifyFlags::WITNESS,
            0,
        ),
        Err(Error::EvalFalse),
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/script_tests.json#L2040
#[test]
fn witness_basic_p2sh_p2wpkh_with_wrong_value() {
    assert_eq!(
        run_witness_test(
            "16001491b24bf9f5288532960ac687abb035127b1d28a5",
            "a91417743beb429c55c942d2ec703b98c4d57c2df5c687",
            vec![
                "304402204fc3a2cd61a47913f2a5f9107d0ad4a504c7b31ee2d6b3b2f38c2b10ee031e940220055d58b7c3c281aaa381d8f486ac0f3e361939acfd568046cb6a311cdfa974cf01",
                "0479be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798483ada7726a3c4655da4fbfc0e1108a8fd17b448a68554199c47d08ffb10d4b8",
            ],
            VerifyFlags::P2SH | VerifyFlags::WITNESS,
            0,
        ),
        Err(Error::EvalFalse),
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/script_tests.json#L2052
#[test]
fn witness_p2wpkh_with_future_version() {
    assert_eq!(
        run_witness_test(
            "",
            "511491b24bf9f5288532960ac687abb035127b1d28a5",
            vec![
                "304402205ae57ae0534c05ca9981c8a6cdf353b505eaacb7375f96681a2d1a4ba6f02f84022056248e68643b7d8ce7c7d128c9f1f348bcab8be15d094ad5cadd24251a28df8001",
                "0479be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798483ada7726a3c4655da4fbfc0e1108a8fd17b448a68554199c47d08ffb10d4b8",
            ],
            VerifyFlags::P2SH | VerifyFlags::WITNESS | VerifyFlags::DISCOURAGE_UPGRADABLE_WITNESS_PROGRAM,
            0,
        ),
        Err(Error::DiscourageUpgradableWitnessProgram),
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/script_tests.json#L2064
#[test]
fn witness_p2wpkh_with_wrong_witness_program_length() {
    assert_eq!(
        run_witness_test(
            "",
            "001fb34b78da162751647974d5cb7410aa428ad339dbf7d1e16e833f68a0cbf1c3",
            vec![
                "3044022064100ca0e2a33332136775a86cd83d0230e58b9aebb889c5ac952abff79a46ef02205f1bf900e022039ad3091bdaf27ac2aef3eae9ed9f190d821d3e508405b9513101",
                "0479be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798483ada7726a3c4655da4fbfc0e1108a8fd17b448a68554199c47d08ffb10d4b8",
            ],
            VerifyFlags::P2SH | VerifyFlags::WITNESS,
            0,
        ),
        Err(Error::WitnessProgramWrongLength),
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/script_tests.json#L2076
#[test]
fn witness_p2wsh_with_empty_witness() {
    assert_eq!(
        run_witness_test(
            "",
            "0020b95237b48faaa69eb078e1170be3b5cbb3fddf16d0a991e14ad274f7b33a4f64",
            vec![],
            VerifyFlags::P2SH | VerifyFlags::WITNESS,
            0,
        ),
        Err(Error::WitnessProgramWitnessEmpty),
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/script_tests.json#L2083
#[test]
fn witness_p2wsh_with_witness_program_mismatch() {
    assert_eq!(
        run_witness_test(
            "",
            "0020b95237b48faaa69eb078e1170be3b5cbb3fddf16d0a991e14ad274f7b33a4f64",
            vec![
                "3044022039105b995a5f448639a997a5c90fda06f50b49df30c3bdb6663217bf79323db002206fecd54269dec569fcc517178880eb58bb40f381a282bb75766ff3637d5f4b4301",
                "400479be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798483ada7726a3c4655da4fbfc0e1108a8fd17b448a68554199c47d08ffb10d4b8ac",
            ],
            VerifyFlags::P2SH | VerifyFlags::WITNESS,
            0,
        ),
        Err(Error::WitnessProgramMismatch),
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/script_tests.json#L2095
#[test]
fn witness_p2wpkh_with_witness_program_mismatch() {
    assert_eq!(
        run_witness_test(
            "",
            "001491b24bf9f5288532960ac687abb035127b1d28a5",
            vec![
                "304402201a96950593cb0af32d080b0f193517f4559241a8ebd1e95e414533ad64a3f423022047f4f6d3095c23235bdff3aeff480d0529c027a3f093cb265b7cbf148553b85101",
                "0479be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798483ada7726a3c4655da4fbfc0e1108a8fd17b448a68554199c47d08ffb10d4b8",
                "",
            ],
            VerifyFlags::P2SH | VerifyFlags::WITNESS ,
            0,
        ),
        Err(Error::WitnessProgramMismatch),
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/script_tests.json#L2108
#[test]
fn witness_p2wpkh_with_non_empty_script_sig() {
    assert_eq!(
        run_witness_test(
            "5b",
            "001491b24bf9f5288532960ac687abb035127b1d28a5",
            vec![
                "304402201a96950593cb0af32d080b0f193517f4559241a8ebd1e95e414533ad64a3f423022047f4f6d3095c23235bdff3aeff480d0529c027a3f093cb265b7cbf148553b85101",
                "0479be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798483ada7726a3c4655da4fbfc0e1108a8fd17b448a68554199c47d08ffb10d4b8",
            ],
            VerifyFlags::P2SH | VerifyFlags::WITNESS ,
            0,
        ),
        Err(Error::WitnessMalleated),
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/script_tests.json#L2120
#[test]
fn witness_p2sh_p2wpkh_with_superfluous_push_in_script_sig() {
    assert_eq!(
        run_witness_test(
            "5b1600147cf9c846cd4882efec4bf07e44ebdad495c94f4b",
            "a9144e0c2aed91315303fc6a1dc4c7bc21c88f75402e87",
            vec![
                "304402204209e49457c2358f80d0256bc24535b8754c14d08840fc4be762d6f5a0aed80b02202eaf7d8fc8d62f60c67adcd99295528d0e491ae93c195cec5a67e7a09532a88001",
                "048282263212c609d9ea2a6e3e172de238d8c39cabd5ac1ca10646e23fd5f5150811f8a8098557dfe45e8256e830b60ace62d613ac2f7b17bed31b6eaff6e26caf",
            ],
            VerifyFlags::P2SH | VerifyFlags::WITNESS ,
            0,
        ),
        Err(Error::WitnessMalleatedP2SH),
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/script_tests.json#L2132
#[test]
fn witness_p2pk_with_witness() {
    assert_eq!(
        run_witness_test(
            "47304402200a5c6163f07b8d3b013c4d1d6dba25e780b39658d79ba37af7057a3b7f15ffa102201fd9b4eaa9943f734928b99a83592c2e7bf342ea2680f6a2bb705167966b742001",
            "410479be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798483ada7726a3c4655da4fbfc0e1108a8fd17b448a68554199c47d08ffb10d4b8ac",
            vec![""],
            VerifyFlags::P2SH | VerifyFlags::WITNESS ,
            0,
        ),
        Err(Error::WitnessUnexpected),
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/script_tests.json#L2299
#[test]
fn witness_p2wsh_checkmultisig() {
    assert_eq!(
        run_witness_test(
            "",
            "002008a6665ebfd43b02323423e764e185d98d1587f903b81507dbb69bfc41005efa",
            vec![
                "",
                "304402202d092ededd1f060609dbf8cb76950634ff42b3e62cf4adb69ab92397b07d742302204ff886f8d0817491a96d1daccdcc820f6feb122ee6230143303100db37dfa79f01",
                "5121038282263212c609d9ea2a6e3e172de238d8c39cabd5ac1ca10646e23fd5f51508410479be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798483ada7726a3c4655da4fbfc0e1108a8fd17b448a68554199c47d08ffb10d4b852ae",
            ],
            VerifyFlags::P2SH | VerifyFlags::WITNESS,
            1,
        ),
        Ok(()),
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/script_tests.json#L2312
#[test]
fn witness_p2sh_p2wsh_checkmultisig() {
    assert_eq!(
        run_witness_test(
            "22002008a6665ebfd43b02323423e764e185d98d1587f903b81507dbb69bfc41005efa",
            "a9146f5ecd4b83b77f3c438f5214eff96454934fc5d187",
            vec![
                "",
                "304402202dd7e91243f2235481ffb626c3b7baf2c859ae3a5a77fb750ef97b99a8125dc002204960de3d3c3ab9496e218ec57e5240e0e10a6f9546316fe240c216d45116d29301",
                "5121038282263212c609d9ea2a6e3e172de238d8c39cabd5ac1ca10646e23fd5f51508410479be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798483ada7726a3c4655da4fbfc0e1108a8fd17b448a68554199c47d08ffb10d4b852ae",
            ],
            VerifyFlags::P2SH | VerifyFlags::WITNESS,
            1,
        ),
        Ok(()),
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/script_tests.json#L2351
#[test]
fn witness_p2wsh_checkmultisig_using_key2() {
    assert_eq!(
        run_witness_test(
            "",
            "002008a6665ebfd43b02323423e764e185d98d1587f903b81507dbb69bfc41005efa",
            vec![
                "",
                "304402201e9e6f7deef5b2f21d8223c5189b7d5e82d237c10e97165dd08f547c4e5ce6ed02206796372eb1cc6acb52e13ee2d7f45807780bf96b132cb6697f69434be74b1af901",
                "5121038282263212c609d9ea2a6e3e172de238d8c39cabd5ac1ca10646e23fd5f51508410479be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798483ada7726a3c4655da4fbfc0e1108a8fd17b448a68554199c47d08ffb10d4b852ae",
            ],
            VerifyFlags::P2SH | VerifyFlags::WITNESS,
            1,
        ),
        Ok(()),
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/script_tests.json#L2364
#[test]
fn witness_p2sh_p2wsh_checkmultisig_using_key2() {
    assert_eq!(
        run_witness_test(
            "22002008a6665ebfd43b02323423e764e185d98d1587f903b81507dbb69bfc41005efa",
            "a9146f5ecd4b83b77f3c438f5214eff96454934fc5d187",
            vec![
                "",
                "3044022045e667f3f0f3147b95597a24babe9afecea1f649fd23637dfa7ed7e9f3ac18440220295748e81005231135289fe3a88338dabba55afa1bdb4478691337009d82b68d01",
                "5121038282263212c609d9ea2a6e3e172de238d8c39cabd5ac1ca10646e23fd5f51508410479be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798483ada7726a3c4655da4fbfc0e1108a8fd17b448a68554199c47d08ffb10d4b852ae",
            ],
            VerifyFlags::P2SH | VerifyFlags::WITNESS,
            1,
        ),
        Ok(()),
    );
}

fn run_witness_test_tx_test(
    script_pubkey: &str,
    tx: &Transaction,
    flags: &VerifyFlags,
    amount: u64,
    input_index: usize,
) -> Result<(), Error> {
    let pk = hex::decode(script_pubkey).unwrap();
    let script_pubkey = Script::from_bytes(&pk);
    let mut checker = TransactionSignatureChecker::new(tx, input_index, amount);

    verify_script(
        &tx.input[input_index].script_sig,
        script_pubkey,
        &tx.input[input_index].witness,
        flags,
        &mut checker,
    )
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_invalid.json#L254
#[test]
fn witness_unknown_program_version() {
    let tx = deserialize_hex("0100000000010300010000000000000000000000000000000000000000000000000000000000000000000000ffffffff00010000000000000000000000000000000000000000000000000000000000000100000000ffffffff00010000000000000000000000000000000000000000000000000000000000000200000000ffffffff03e8030000000000000151d0070000000000000151b80b00000000000001510002483045022100a3cec69b52cba2d2de623ffffffffff1606184ea55476c0f8189fda231bc9cbb022003181ad597f7c380a7d1c740286b1d022b8b04ded028b833282e055e03b8efef812103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc710000000000").unwrap();
    let flags = VerifyFlags::P2SH
        | VerifyFlags::WITNESS
        | VerifyFlags::DISCOURAGE_UPGRADABLE_WITNESS_PROGRAM;
    assert_eq!(
        run_witness_test_tx_test("51", &tx, &flags, 1000, 0)
            .and_then(|_| {
                run_witness_test_tx_test(
                    "60144c9c3dfac4207d5d8cb89df5722cb3d712385e3f",
                    &tx,
                    &flags,
                    2000,
                    1,
                )
            })
            .and_then(|_| run_witness_test_tx_test("51", &tx, &flags, 3000, 2)),
        Err(Error::DiscourageUpgradableWitnessProgram),
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_invalid.json#L260
#[test]
fn witness_unknown_program0_lengh() {
    let tx = deserialize_hex::<Transaction>("0100000000010300010000000000000000000000000000000000000000000000000000000000000000000000ffffffff00010000000000000000000000000000000000000000000000000000000000000100000000ffffffff00010000000000000000000000000000000000000000000000000000000000000200000000ffffffff04b60300000000000001519e070000000000000151860b0000000000000100960000000000000001510002473044022022fceb54f62f8feea77faac7083c3b56c4676a78f93745adc8a35800bc36adfa022026927df9abcf0a8777829bcfcce3ff0a385fa54c3f9df577405e3ef24ee56479022103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc710000000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        Err(Error::WitnessProgramWrongLength),
        run_witness_test_tx_test("51", &tx, &flags, 1000, 0)
            .and_then(|_| run_witness_test_tx_test(
                "00154c9c3dfac4207d5d8cb89df5722cb3d712385e3fff",
                &tx,
                &flags,
                2000,
                1
            ))
            .and_then(|_| run_witness_test_tx_test("51", &tx, &flags, 3000, 2))
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_invalid.json#L260
#[test]
fn witness_single_anyone_same_index_value_changed() {
    let tx = deserialize_hex::<Transaction>("0100000000010300010000000000000000000000000000000000000000000000000000000000000000000000ffffffff00010000000000000000000000000000000000000000000000000000000000000100000000ffffffff00010000000000000000000000000000000000000000000000000000000000000200000000ffffffff03e80300000000000001516c070000000000000151b80b0000000000000151000248304502210092f4777a0f17bf5aeb8ae768dec5f2c14feabf9d1fe2c89c78dfed0f13fdb86902206da90a86042e252bcd1e80a168c719e4a1ddcc3cebea24b9812c5453c79107e9832103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc710000000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        Err(Error::EvalFalse),
        run_witness_test_tx_test("51", &tx, &flags, 1000, 0)
            .and_then(|_| run_witness_test_tx_test(
                "00144c9c3dfac4207d5d8cb89df5722cb3d712385e3f",
                &tx,
                &flags,
                2000,
                1
            ))
            .and_then(|_| run_witness_test_tx_test("51", &tx, &flags, 3000, 2))
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_invalid.json#L272
#[test]
fn witness_none_anyone_same_index_value_changed() {
    let tx = deserialize_hex::<Transaction>("0100000000010300010000000000000000000000000000000000000000000000000000000000000000000000ffffffff000100000000000000000000000000000000000000000000000000000000000001000000000100000000010000000000000000000000000000000000000000000000000000000000000200000000ffffffff00000248304502210091b32274295c2a3fa02f5bce92fb2789e3fc6ea947fbe1a76e52ea3f4ef2381a022079ad72aefa3837a2e0c033a8652a59731da05fa4a813f4fc48e87c075037256b822103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc710000000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        Err(Error::EvalFalse),
        run_witness_test_tx_test("51", &tx, &flags, 1000, 0)
            .and_then(|_| run_witness_test_tx_test(
                "00144c9c3dfac4207d5d8cb89df5722cb3d712385e3f",
                &tx,
                &flags,
                2000,
                1
            ))
            .and_then(|_| run_witness_test_tx_test("51", &tx, &flags, 3000, 2))
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_invalid.json#L278
#[test]
fn witness_all_anyone_third_value_changed() {
    let tx = deserialize_hex::<Transaction>("0100000000010300010000000000000000000000000000000000000000000000000000000000000000000000ffffffff00010000000000000000000000000000000000000000000000000000000000000100000000ffffffff00010000000000000000000000000000000000000000000000000000000000000200000000ffffffff03e8030000000000000151d0070000000000000151540b00000000000001510002483045022100a3cec69b52cba2d2de623eeef89e0ba1606184ea55476c0f8189fda231bc9cbb022003181ad597f7c380a7d1c740286b1d022b8b04ded028b833282e055e03b8efef812103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc710000000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        Err(Error::EvalFalse),
        run_witness_test_tx_test("51", &tx, &flags, 1000, 0)
            .and_then(|_| run_witness_test_tx_test(
                "00144c9c3dfac4207d5d8cb89df5722cb3d712385e3f",
                &tx,
                &flags,
                2000,
                1
            ))
            .and_then(|_| run_witness_test_tx_test("51", &tx, &flags, 3000, 2))
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_invalid.json#L284
#[test]
fn witness_with_push_of_521_bytes() {
    let tx = deserialize_hex::<Transaction>("0100000000010100010000000000000000000000000000000000000000000000000000000000000000000000ffffffff010000000000000000015102fd0902000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002755100000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        Err(Error::PushSize),
        run_witness_test_tx_test(
            "002033198a9bfef674ebddb9ffaa52928017b8472791e54c609cb95f278ac6b1e349",
            &tx,
            &flags,
            1000,
            0
        )
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_invalid.json#L288
#[test]
fn witness_unknown_version_with_false_on_stack() {
    let tx = deserialize_hex::<Transaction>("0100000000010100010000000000000000000000000000000000000000000000000000000000000000000000ffffffff010000000000000000015101010100000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        Err(Error::EvalFalse),
        run_witness_test_tx_test("60020000", &tx, &flags, 2000, 0)
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_invalid.json#L292
#[test]
fn witness_unknown_version_with_non_empty_stack() {
    let tx = deserialize_hex::<Transaction>("0100000000010100010000000000000000000000000000000000000000000000000000000000000000000000ffffffff01000000000000000001510102515100000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    // Error::EvalFalse is expected in parity-bitcoin, which has been corrected to be Error::CleanStack in https://github.com/bitcoin/bitcoin/pull/12167.
    assert_eq!(
        Err(Error::CleanStack),
        run_witness_test_tx_test(
            "00202f04a3aa051f1f60d695f6c44c0c3d383973dfd446ace8962664a76bb10e31a8",
            &tx,
            &flags,
            2000,
            0
        )
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_invalid.json#L296
#[test]
fn witness_program0_with_push_of_2_bytes() {
    let tx = deserialize_hex::<Transaction>("0100000000010100010000000000000000000000000000000000000000000000000000000000000000000000ffffffff010000000000000000015101040002000100000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        Err(Error::WitnessProgramWrongLength),
        run_witness_test_tx_test("00020001", &tx, &flags, 2000, 0)
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_invalid.json#L300
#[test]
fn witness_unknown_version_with_non_empty_script_sig() {
    let tx = deserialize_hex::<Transaction>("01000000010001000000000000000000000000000000000000000000000000000000000000000000000151ffffffff010000000000000000015100000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        Err(Error::WitnessMalleated),
        run_witness_test_tx_test("60020001", &tx, &flags, 2000, 0)
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_invalid.json#L304
#[test]
fn witness_non_witness_single_anyone_hash_input_position() {
    let tx = deserialize_hex::<Transaction>("010000000200010000000000000000000000000000000000000000000000000000000000000100000049483045022100acb96cfdbda6dc94b489fd06f2d720983b5f350e31ba906cdbd800773e80b21c02200d74ea5bdf114212b4bbe9ed82c36d2e369e302dff57cb60d01c428f0bd3daab83ffffffff0001000000000000000000000000000000000000000000000000000000000000000000004847304402202a0b4b1294d70540235ae033d78e64b4897ec859c7b6f1b2b1d8a02e1d46006702201445e756d2254b0f1dfda9ab8e1e1bc26df9668077403204f32d16a49a36eb6983ffffffff02e9030000000000000151e803000000000000015100000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        Err(Error::EvalFalse),
        run_witness_test_tx_test(
            "2103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc71ac",
            &tx,
            &flags,
            1000,
            0
        )
        .and_then(|_| run_witness_test_tx_test(
            "2103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc71ac",
            &tx,
            &flags,
            1001,
            1
        ))
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_invalid.json#L313
#[test]
fn witness_33_bytes_witness_script_pubkey() {
    let tx = deserialize_hex::<Transaction>("010000000100010000000000000000000000000000000000000000000000000000000000000000000000ffffffff01e803000000000000015100000000").unwrap();
    let flags = VerifyFlags::P2SH
        | VerifyFlags::WITNESS
        | VerifyFlags::DISCOURAGE_UPGRADABLE_WITNESS_PROGRAM;
    assert_eq!(
        Err(Error::DiscourageUpgradableWitnessProgram),
        run_witness_test_tx_test(
            "6021ff25429251b5a84f452230a3c75fd886b7fc5a7865ce4a7bb7a9d7c5be6da3dbff",
            &tx,
            &flags,
            1000,
            0
        )
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_valid.json#L320
#[test]
fn witness_valid_p2wpkh() {
    let tx = deserialize_hex::<Transaction>("0100000000010100010000000000000000000000000000000000000000000000000000000000000000000000ffffffff01e8030000000000001976a9144c9c3dfac4207d5d8cb89df5722cb3d712385e3f88ac02483045022100cfb07164b36ba64c1b1e8c7720a56ad64d96f6ef332d3d37f9cb3c96477dc44502200a464cd7a9cf94cd70f66ce4f4f0625ef650052c7afcfe29d7d7e01830ff91ed012103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc7100000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        Ok(()),
        run_witness_test_tx_test(
            "00144c9c3dfac4207d5d8cb89df5722cb3d712385e3f",
            &tx,
            &flags,
            1000,
            0
        )
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_valid.json#L324
#[test]
fn witness_valid_p2wsh() {
    let tx = deserialize_hex::<Transaction>("0100000000010100010000000000000000000000000000000000000000000000000000000000000000000000ffffffff01e8030000000000001976a9144c9c3dfac4207d5d8cb89df5722cb3d712385e3f88ac02483045022100aa5d8aa40a90f23ce2c3d11bc845ca4a12acd99cbea37de6b9f6d86edebba8cb022022dedc2aa0a255f74d04c0b76ece2d7c691f9dd11a64a8ac49f62a99c3a05f9d01232103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc71ac00000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        Ok(()),
        run_witness_test_tx_test(
            "0020ff25429251b5a84f452230a3c75fd886b7fc5a7865ce4a7bb7a9d7c5be6da3db",
            &tx,
            &flags,
            1000,
            0
        )
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_valid.json#L328
#[test]
fn witness_valid_p2sh_p2wpkh() {
    let tx = deserialize_hex::<Transaction>("01000000000101000100000000000000000000000000000000000000000000000000000000000000000000171600144c9c3dfac4207d5d8cb89df5722cb3d712385e3fffffffff01e8030000000000001976a9144c9c3dfac4207d5d8cb89df5722cb3d712385e3f88ac02483045022100cfb07164b36ba64c1b1e8c7720a56ad64d96f6ef332d3d37f9cb3c96477dc44502200a464cd7a9cf94cd70f66ce4f4f0625ef650052c7afcfe29d7d7e01830ff91ed012103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc7100000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        Ok(()),
        run_witness_test_tx_test(
            "a914fe9c7dacc9fcfbf7e3b7d5ad06aa2b28c5a7b7e387",
            &tx,
            &flags,
            1000,
            0
        )
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_valid.json#L328
#[test]
fn witness_valid_p2sh_p2wsh() {
    let tx = deserialize_hex::<Transaction>("0100000000010100010000000000000000000000000000000000000000000000000000000000000000000023220020ff25429251b5a84f452230a3c75fd886b7fc5a7865ce4a7bb7a9d7c5be6da3dbffffffff01e8030000000000001976a9144c9c3dfac4207d5d8cb89df5722cb3d712385e3f88ac02483045022100aa5d8aa40a90f23ce2c3d11bc845ca4a12acd99cbea37de6b9f6d86edebba8cb022022dedc2aa0a255f74d04c0b76ece2d7c691f9dd11a64a8ac49f62a99c3a05f9d01232103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc71ac00000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        Ok(()),
        run_witness_test_tx_test(
            "a9142135ab4f0981830311e35600eebc7376dce3a91487",
            &tx,
            &flags,
            1000,
            0
        )
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_valid.json#L328
#[test]
fn witness_valid_single_anyoune() {
    let tx = deserialize_hex::<Transaction>("0100000000010400010000000000000000000000000000000000000000000000000000000000000200000000ffffffff00010000000000000000000000000000000000000000000000000000000000000100000000ffffffff00010000000000000000000000000000000000000000000000000000000000000000000000ffffffff00010000000000000000000000000000000000000000000000000000000000000300000000ffffffff05540b0000000000000151d0070000000000000151840300000000000001513c0f00000000000001512c010000000000000151000248304502210092f4777a0f17bf5aeb8ae768dec5f2c14feabf9d1fe2c89c78dfed0f13fdb86902206da90a86042e252bcd1e80a168c719e4a1ddcc3cebea24b9812c5453c79107e9832103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc71000000000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        Ok(()),
        run_witness_test_tx_test("51", &tx, &flags, 3100, 0)
            .and_then(|_| run_witness_test_tx_test(
                "00144c9c3dfac4207d5d8cb89df5722cb3d712385e3f",
                &tx,
                &flags,
                2000,
                1
            ))
            .and_then(|_| run_witness_test_tx_test("51", &tx, &flags, 1100, 2))
            .and_then(|_| run_witness_test_tx_test("51", &tx, &flags, 4100, 2))
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_valid.json#L343
#[test]
fn witness_valid_single_anyoune_same_signature() {
    let tx = deserialize_hex::<Transaction>("0100000000010400010000000000000000000000000000000000000000000000000000000000000200000000ffffffff00010000000000000000000000000000000000000000000000000000000000000100000000ffffffff00010000000000000000000000000000000000000000000000000000000000000000000000ffffffff00010000000000000000000000000000000000000000000000000000000000000300000000ffffffff05540b0000000000000151d0070000000000000151840300000000000001513c0f00000000000001512c010000000000000151000248304502210092f4777a0f17bf5aeb8ae768dec5f2c14feabf9d1fe2c89c78dfed0f13fdb86902206da90a86042e252bcd1e80a168c719e4a1ddcc3cebea24b9812c5453c79107e9832103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc71000000000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        Ok(()),
        run_witness_test_tx_test("51", &tx, &flags, 3100, 0)
            .and_then(|_| run_witness_test_tx_test(
                "00144c9c3dfac4207d5d8cb89df5722cb3d712385e3f",
                &tx,
                &flags,
                2000,
                1
            ))
            .and_then(|_| run_witness_test_tx_test("51", &tx, &flags, 1100, 2))
            .and_then(|_| run_witness_test_tx_test("51", &tx, &flags, 4100, 2))
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_valid.json#L349
#[test]
fn witness_valid_single() {
    let tx = deserialize_hex::<Transaction>("0100000000010300010000000000000000000000000000000000000000000000000000000000000000000000ffffffff00010000000000000000000000000000000000000000000000000000000000000100000000ffffffff00010000000000000000000000000000000000000000000000000000000000000200000000ffffffff0484030000000000000151d0070000000000000151540b0000000000000151c800000000000000015100024730440220699e6b0cfe015b64ca3283e6551440a34f901ba62dd4c72fe1cb815afb2e6761022021cc5e84db498b1479de14efda49093219441adc6c543e5534979605e273d80b032103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc710000000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        Ok(()),
        run_witness_test_tx_test("51", &tx, &flags, 1000, 0)
            .and_then(|_| run_witness_test_tx_test(
                "00144c9c3dfac4207d5d8cb89df5722cb3d712385e3f",
                &tx,
                &flags,
                2000,
                1
            ))
            .and_then(|_| run_witness_test_tx_test("51", &tx, &flags, 3000, 2))
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_valid.json#L355
#[test]
fn witness_valid_single_same_signature() {
    let tx = deserialize_hex::<Transaction>("0100000000010300010000000000000000000000000000000000000000000000000000000000000000000000ffffffff00010000000000000000000000000000000000000000000000000000000000000100000000ffffffff00010000000000000000000000000000000000000000000000000000000000000200000000ffffffff03e8030000000000000151d0070000000000000151b80b000000000000015100024730440220699e6b0cfe015b64ca3283e6551440a34f901ba62dd4c72fe1cb815afb2e6761022021cc5e84db498b1479de14efda49093219441adc6c543e5534979605e273d80b032103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc710000000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        Ok(()),
        run_witness_test_tx_test("51", &tx, &flags, 1000, 0)
            .and_then(|_| run_witness_test_tx_test(
                "00144c9c3dfac4207d5d8cb89df5722cb3d712385e3f",
                &tx,
                &flags,
                2000,
                1
            ))
            .and_then(|_| run_witness_test_tx_test("51", &tx, &flags, 3000, 2))
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_valid.json#L361
#[test]
fn witness_valid_none_anyone() {
    let tx = deserialize_hex::<Transaction>("0100000000010400010000000000000000000000000000000000000000000000000000000000000200000000ffffffff00010000000000000000000000000000000000000000000000000000000000000000000000ffffffff00010000000000000000000000000000000000000000000000000000000000000100000000ffffffff00010000000000000000000000000000000000000000000000000000000000000300000000ffffffff04b60300000000000001519e070000000000000151860b00000000000001009600000000000000015100000248304502210091b32274295c2a3fa02f5bce92fb2789e3fc6ea947fbe1a76e52ea3f4ef2381a022079ad72aefa3837a2e0c033a8652a59731da05fa4a813f4fc48e87c075037256b822103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc710000000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        Ok(()),
        run_witness_test_tx_test("51", &tx, &flags, 3100, 0)
            .and_then(|_| run_witness_test_tx_test("51", &tx, &flags, 1100, 1))
            .and_then(|_| run_witness_test_tx_test(
                "00144c9c3dfac4207d5d8cb89df5722cb3d712385e3f",
                &tx,
                &flags,
                2000,
                2
            ))
            .and_then(|_| run_witness_test_tx_test("51", &tx, &flags, 4100, 3))
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_valid.json#L368
#[test]
fn witness_valid_none_anyone_same_signature() {
    let tx = deserialize_hex::<Transaction>("0100000000010300010000000000000000000000000000000000000000000000000000000000000000000000ffffffff00010000000000000000000000000000000000000000000000000000000000000100000000ffffffff00010000000000000000000000000000000000000000000000000000000000000200000000ffffffff03e8030000000000000151d0070000000000000151b80b0000000000000151000248304502210091b32274295c2a3fa02f5bce92fb2789e3fc6ea947fbe1a76e52ea3f4ef2381a022079ad72aefa3837a2e0c033a8652a59731da05fa4a813f4fc48e87c075037256b822103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc710000000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        Ok(()),
        run_witness_test_tx_test("51", &tx, &flags, 1000, 0)
            .and_then(|_| run_witness_test_tx_test(
                "00144c9c3dfac4207d5d8cb89df5722cb3d712385e3f",
                &tx,
                &flags,
                2000,
                1
            ))
            .and_then(|_| run_witness_test_tx_test("51", &tx, &flags, 3000, 2))
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_valid.json#L374
#[test]
fn witness_none() {
    let tx = deserialize_hex::<Transaction>("0100000000010300010000000000000000000000000000000000000000000000000000000000000000000000ffffffff00010000000000000000000000000000000000000000000000000000000000000100000000ffffffff00010000000000000000000000000000000000000000000000000000000000000200000000ffffffff04b60300000000000001519e070000000000000151860b0000000000000100960000000000000001510002473044022022fceb54f62f8feea77faac7083c3b56c4676a78f93745adc8a35800bc36adfa022026927df9abcf0a8777829bcfcce3ff0a385fa54c3f9df577405e3ef24ee56479022103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc710000000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        Ok(()),
        run_witness_test_tx_test("51", &tx, &flags, 1000, 0)
            .and_then(|_| run_witness_test_tx_test(
                "00144c9c3dfac4207d5d8cb89df5722cb3d712385e3f",
                &tx,
                &flags,
                2000,
                1
            ))
            .and_then(|_| run_witness_test_tx_test("51", &tx, &flags, 3000, 2))
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_valid.json#L380
#[test]
fn witness_none_same_signature() {
    let tx = deserialize_hex::<Transaction>("0100000000010300010000000000000000000000000000000000000000000000000000000000000000000000ffffffff00010000000000000000000000000000000000000000000000000000000000000100000000ffffffff00010000000000000000000000000000000000000000000000000000000000000200000000ffffffff03e8030000000000000151d0070000000000000151b80b00000000000001510002473044022022fceb54f62f8feea77faac7083c3b56c4676a78f93745adc8a35800bc36adfa022026927df9abcf0a8777829bcfcce3ff0a385fa54c3f9df577405e3ef24ee56479022103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc710000000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        Ok(()),
        run_witness_test_tx_test("51", &tx, &flags, 1000, 0)
            .and_then(|_| run_witness_test_tx_test(
                "00144c9c3dfac4207d5d8cb89df5722cb3d712385e3f",
                &tx,
                &flags,
                2000,
                1
            ))
            .and_then(|_| run_witness_test_tx_test("51", &tx, &flags, 3000, 2))
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_valid.json#L386
#[test]
fn witness_none_same_signature_sequence_changed() {
    let tx = deserialize_hex::<Transaction>("01000000000103000100000000000000000000000000000000000000000000000000000000000000000000000200000000010000000000000000000000000000000000000000000000000000000000000100000000ffffffff000100000000000000000000000000000000000000000000000000000000000002000000000200000003e8030000000000000151d0070000000000000151b80b00000000000001510002473044022022fceb54f62f8feea77faac7083c3b56c4676a78f93745adc8a35800bc36adfa022026927df9abcf0a8777829bcfcce3ff0a385fa54c3f9df577405e3ef24ee56479022103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc710000000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        Ok(()),
        run_witness_test_tx_test("51", &tx, &flags, 1000, 0)
            .and_then(|_| run_witness_test_tx_test(
                "00144c9c3dfac4207d5d8cb89df5722cb3d712385e3f",
                &tx,
                &flags,
                2000,
                1
            ))
            .and_then(|_| run_witness_test_tx_test("51", &tx, &flags, 3000, 2))
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_valid.json#L392
#[test]
fn witness_all_anyone() {
    let tx = deserialize_hex::<Transaction>("0100000000010400010000000000000000000000000000000000000000000000000000000000000200000000ffffffff00010000000000000000000000000000000000000000000000000000000000000000000000ffffffff00010000000000000000000000000000000000000000000000000000000000000100000000ffffffff00010000000000000000000000000000000000000000000000000000000000000300000000ffffffff03e8030000000000000151d0070000000000000151b80b0000000000000151000002483045022100a3cec69b52cba2d2de623eeef89e0ba1606184ea55476c0f8189fda231bc9cbb022003181ad597f7c380a7d1c740286b1d022b8b04ded028b833282e055e03b8efef812103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc710000000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        Ok(()),
        run_witness_test_tx_test("51", &tx, &flags, 3100, 0)
            .and_then(|_| run_witness_test_tx_test("51", &tx, &flags, 1100, 1))
            .and_then(|_| run_witness_test_tx_test(
                "00144c9c3dfac4207d5d8cb89df5722cb3d712385e3f",
                &tx,
                &flags,
                2000,
                2
            ))
            .and_then(|_| run_witness_test_tx_test("51", &tx, &flags, 4100, 3))
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_valid.json#L399
#[test]
fn witness_all_anyone_same_signature() {
    let tx = deserialize_hex::<Transaction>("0100000000010300010000000000000000000000000000000000000000000000000000000000000000000000ffffffff00010000000000000000000000000000000000000000000000000000000000000100000000ffffffff00010000000000000000000000000000000000000000000000000000000000000200000000ffffffff03e8030000000000000151d0070000000000000151b80b00000000000001510002483045022100a3cec69b52cba2d2de623eeef89e0ba1606184ea55476c0f8189fda231bc9cbb022003181ad597f7c380a7d1c740286b1d022b8b04ded028b833282e055e03b8efef812103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc710000000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        Ok(()),
        run_witness_test_tx_test("51", &tx, &flags, 1000, 0)
            .and_then(|_| run_witness_test_tx_test(
                "00144c9c3dfac4207d5d8cb89df5722cb3d712385e3f",
                &tx,
                &flags,
                2000,
                1
            ))
            .and_then(|_| run_witness_test_tx_test("51", &tx, &flags, 3000, 2))
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_valid.json#L405
#[test]
fn witness_unknown_witness_program_version() {
    let tx = deserialize_hex::<Transaction>("0100000000010300010000000000000000000000000000000000000000000000000000000000000000000000ffffffff00010000000000000000000000000000000000000000000000000000000000000100000000ffffffff00010000000000000000000000000000000000000000000000000000000000000200000000ffffffff03e8030000000000000151d0070000000000000151b80b00000000000001510002483045022100a3cec69b52cba2d2de623ffffffffff1606184ea55476c0f8189fda231bc9cbb022003181ad597f7c380a7d1c740286b1d022b8b04ded028b833282e055e03b8efef812103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc710000000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        Ok(()),
        run_witness_test_tx_test("51", &tx, &flags, 1000, 0)
            .and_then(|_| run_witness_test_tx_test(
                "60144c9c3dfac4207d5d8cb89df5722cb3d712385e3f",
                &tx,
                &flags,
                2000,
                1
            ))
            .and_then(|_| run_witness_test_tx_test("51", &tx, &flags, 3000, 2))
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_valid.json#L411
#[test]
fn witness_push_520_bytes() {
    let tx = deserialize_hex::<Transaction>("0100000000010100010000000000000000000000000000000000000000000000000000000000000000000000ffffffff010000000000000000015102fd08020000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002755100000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        Ok(()),
        run_witness_test_tx_test(
            "002033198a9bfef674ebddb9ffaa52928017b8472791e54c609cb95f278ac6b1e349",
            &tx,
            &flags,
            1000,
            0
        )
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_valid.json#L415
#[test]
fn witness_mixed_transaction() {
    let tx = deserialize_hex::<Transaction>("0100000000010c00010000000000000000000000000000000000000000000000000000000000000000000000ffffffff00010000000000000000000000000000000000000000000000000000000000000100000000ffffffff0001000000000000000000000000000000000000000000000000000000000000020000006a473044022026c2e65b33fcd03b2a3b0f25030f0244bd23cc45ae4dec0f48ae62255b1998a00220463aa3982b718d593a6b9e0044513fd67a5009c2fdccc59992cffc2b167889f4012103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc71ffffffff0001000000000000000000000000000000000000000000000000000000000000030000006a4730440220008bd8382911218dcb4c9f2e75bf5c5c3635f2f2df49b36994fde85b0be21a1a02205a539ef10fb4c778b522c1be852352ea06c67ab74200977c722b0bc68972575a012103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc71ffffffff0001000000000000000000000000000000000000000000000000000000000000040000006b483045022100d9436c32ff065127d71e1a20e319e4fe0a103ba0272743dbd8580be4659ab5d302203fd62571ee1fe790b182d078ecfd092a509eac112bea558d122974ef9cc012c7012103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc71ffffffff0001000000000000000000000000000000000000000000000000000000000000050000006a47304402200e2c149b114ec546015c13b2b464bbcb0cdc5872e6775787527af6cbc4830b6c02207e9396c6979fb15a9a2b96ca08a633866eaf20dc0ff3c03e512c1d5a1654f148012103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc71ffffffff0001000000000000000000000000000000000000000000000000000000000000060000006b483045022100b20e70d897dc15420bccb5e0d3e208d27bdd676af109abbd3f88dbdb7721e6d6022005836e663173fbdfe069f54cde3c2decd3d0ea84378092a5d9d85ec8642e8a41012103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc71ffffffff00010000000000000000000000000000000000000000000000000000000000000700000000ffffffff00010000000000000000000000000000000000000000000000000000000000000800000000ffffffff00010000000000000000000000000000000000000000000000000000000000000900000000ffffffff00010000000000000000000000000000000000000000000000000000000000000a00000000ffffffff00010000000000000000000000000000000000000000000000000000000000000b0000006a47304402206639c6e05e3b9d2675a7f3876286bdf7584fe2bbd15e0ce52dd4e02c0092cdc60220757d60b0a61fc95ada79d23746744c72bac1545a75ff6c2c7cdb6ae04e7e9592012103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc71ffffffff0ce8030000000000000151e9030000000000000151ea030000000000000151eb030000000000000151ec030000000000000151ed030000000000000151ee030000000000000151ef030000000000000151f0030000000000000151f1030000000000000151f2030000000000000151f30300000000000001510248304502210082219a54f61bf126bfc3fa068c6e33831222d1d7138c6faa9d33ca87fd4202d6022063f9902519624254d7c2c8ea7ba2d66ae975e4e229ae38043973ec707d5d4a83012103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc7102473044022017fb58502475848c1b09f162cb1688d0920ff7f142bed0ef904da2ccc88b168f02201798afa61850c65e77889cbcd648a5703b487895517c88f85cdd18b021ee246a012103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc7100000000000247304402202830b7926e488da75782c81a54cd281720890d1af064629ebf2e31bf9f5435f30220089afaa8b455bbeb7d9b9c3fe1ed37d07685ade8455c76472cda424d93e4074a012103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc7102473044022026326fcdae9207b596c2b05921dbac11d81040c4d40378513670f19d9f4af893022034ecd7a282c0163b89aaa62c22ec202cef4736c58cd251649bad0d8139bcbf55012103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc71024730440220214978daeb2f38cd426ee6e2f44131a33d6b191af1c216247f1dd7d74c16d84a02205fdc05529b0bc0c430b4d5987264d9d075351c4f4484c16e91662e90a72aab24012103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc710247304402204a6e9f199dc9672cf2ff8094aaa784363be1eb62b679f7ff2df361124f1dca3302205eeb11f70fab5355c9c8ad1a0700ea355d315e334822fa182227e9815308ee8f012103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc710000000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        Ok(()),
        run_witness_test_tx_test(
            "00144c9c3dfac4207d5d8cb89df5722cb3d712385e3f",
            &tx,
            &flags,
            1000,
            0
        )
        .and_then(|_| run_witness_test_tx_test(
            "00144c9c3dfac4207d5d8cb89df5722cb3d712385e3f",
            &tx,
            &flags,
            1001,
            1
        ))
        .and_then(|_| run_witness_test_tx_test(
            "76a9144c9c3dfac4207d5d8cb89df5722cb3d712385e3f88ac",
            &tx,
            &flags,
            1002,
            2
        ))
        .and_then(|_| run_witness_test_tx_test(
            "76a9144c9c3dfac4207d5d8cb89df5722cb3d712385e3f88ac",
            &tx,
            &flags,
            1003,
            3
        ))
        .and_then(|_| run_witness_test_tx_test(
            "76a9144c9c3dfac4207d5d8cb89df5722cb3d712385e3f88ac",
            &tx,
            &flags,
            1004,
            4
        ))
        .and_then(|_| run_witness_test_tx_test(
            "76a9144c9c3dfac4207d5d8cb89df5722cb3d712385e3f88ac",
            &tx,
            &flags,
            1005,
            5
        ))
        .and_then(|_| run_witness_test_tx_test(
            "76a9144c9c3dfac4207d5d8cb89df5722cb3d712385e3f88ac",
            &tx,
            &flags,
            1006,
            6
        ))
        .and_then(|_| run_witness_test_tx_test(
            "00144c9c3dfac4207d5d8cb89df5722cb3d712385e3f",
            &tx,
            &flags,
            1007,
            7
        ))
        .and_then(|_| run_witness_test_tx_test(
            "00144c9c3dfac4207d5d8cb89df5722cb3d712385e3f",
            &tx,
            &flags,
            1008,
            8
        ))
        .and_then(|_| run_witness_test_tx_test(
            "00144c9c3dfac4207d5d8cb89df5722cb3d712385e3f",
            &tx,
            &flags,
            1009,
            9
        ))
        .and_then(|_| run_witness_test_tx_test(
            "00144c9c3dfac4207d5d8cb89df5722cb3d712385e3f",
            &tx,
            &flags,
            1010,
            10
        ))
        .and_then(|_| run_witness_test_tx_test(
            "76a9144c9c3dfac4207d5d8cb89df5722cb3d712385e3f88ac",
            &tx,
            &flags,
            1011,
            11
        ))
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_valid.json#L430
#[test]
fn witness_unknown_version_with_empty_witness() {
    let tx = deserialize_hex::<Transaction>("010000000100010000000000000000000000000000000000000000000000000000000000000000000000ffffffff01e803000000000000015100000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        Ok(()),
        run_witness_test_tx_test(
            "60144c9c3dfac4207d5d8cb89df5722cb3d712385e3f",
            &tx,
            &flags,
            1000,
            0
        )
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_valid.json#L434
#[test]
fn witness_single_output_oob() {
    let tx = deserialize_hex::<Transaction>("0100000000010200010000000000000000000000000000000000000000000000000000000000000000000000ffffffff00010000000000000000000000000000000000000000000000000000000000000100000000ffffffff01d00700000000000001510003483045022100e078de4e96a0e05dcdc0a414124dd8475782b5f3f0ed3f607919e9a5eeeb22bf02201de309b3a3109adb3de8074b3610d4cf454c49b61247a2779a0bcbf31c889333032103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc711976a9144c9c3dfac4207d5d8cb89df5722cb3d712385e3f88ac00000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        Ok(()),
        run_witness_test_tx_test("51", &tx, &flags, 1000, 0).and_then(|_| {
            run_witness_test_tx_test(
                "00204d6c2a32c87821d68fc016fca70797abdb80df6cd84651d40a9300c6bad79e62",
                &tx,
                &flags,
                1000,
                1,
            )
        })
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_valid.json#L439
#[test]
fn witness_1_byte_push_not_witness_script_pubkey() {
    let tx = deserialize_hex::<Transaction>("010000000100010000000000000000000000000000000000000000000000000000000000000000000000ffffffff01e803000000000000015100000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        Ok(()),
        run_witness_test_tx_test("600101", &tx, &flags, 1000, 0)
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_valid.json#L443
#[test]
fn witness_41_byte_push_not_witness_script_pubkey() {
    let tx = deserialize_hex::<Transaction>("010000000100010000000000000000000000000000000000000000000000000000000000000000000000ffffffff01e803000000000000015100000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(Ok(()), run_witness_test_tx_test("6029ff25429251b5a84f452230a3c75fd886b7fc5a7865ce4a7bb7a9d7c5be6da3dbff0000000000000000", &tx, &flags, 1000, 0));
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_valid.json#L447
#[test]
fn witness_version_must_use_op1_to_op16() {
    let tx = deserialize_hex::<Transaction>("010000000100010000000000000000000000000000000000000000000000000000000000000000000000ffffffff01e803000000000000015100000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        Ok(()),
        run_witness_test_tx_test("0110020001", &tx, &flags, 1000, 0)
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_valid.json#L451
#[test]
fn witness_program_push_must_be_canonical() {
    let tx = deserialize_hex::<Transaction>("010000000100010000000000000000000000000000000000000000000000000000000000000000000000ffffffff01e803000000000000015100000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        Ok(()),
        run_witness_test_tx_test("604c020001", &tx, &flags, 1000, 0)
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_valid.json#L455
#[test]
fn witness_single_anyone_does_not_hash_input_position() {
    let tx = deserialize_hex::<Transaction>("0100000000010200010000000000000000000000000000000000000000000000000000000000000000000000ffffffff00010000000000000000000000000000000000000000000000000000000000000100000000ffffffff02e8030000000000000151e90300000000000001510247304402206d59682663faab5e4cb733c562e22cdae59294895929ec38d7c016621ff90da0022063ef0af5f970afe8a45ea836e3509b8847ed39463253106ac17d19c437d3d56b832103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc710248304502210085001a820bfcbc9f9de0298af714493f8a37b3b354bfd21a7097c3e009f2018c022050a8b4dbc8155d4d04da2f5cdd575dcf8dd0108de8bec759bd897ea01ecb3af7832103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc7100000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        Ok(()),
        run_witness_test_tx_test(
            "00144c9c3dfac4207d5d8cb89df5722cb3d712385e3f",
            &tx,
            &flags,
            1000,
            0
        )
        .and_then(|_| run_witness_test_tx_test(
            "00144c9c3dfac4207d5d8cb89df5722cb3d712385e3f",
            &tx,
            &flags,
            1001,
            1
        ))
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_valid.json#L460
#[test]
fn witness_single_anyone_does_not_hash_input_position_permutation() {
    let tx = deserialize_hex::<Transaction>("0100000000010200010000000000000000000000000000000000000000000000000000000000000100000000ffffffff00010000000000000000000000000000000000000000000000000000000000000000000000ffffffff02e9030000000000000151e80300000000000001510248304502210085001a820bfcbc9f9de0298af714493f8a37b3b354bfd21a7097c3e009f2018c022050a8b4dbc8155d4d04da2f5cdd575dcf8dd0108de8bec759bd897ea01ecb3af7832103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc710247304402206d59682663faab5e4cb733c562e22cdae59294895929ec38d7c016621ff90da0022063ef0af5f970afe8a45ea836e3509b8847ed39463253106ac17d19c437d3d56b832103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc7100000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        Ok(()),
        run_witness_test_tx_test(
            "00144c9c3dfac4207d5d8cb89df5722cb3d712385e3f",
            &tx,
            &flags,
            1001,
            0
        )
        .and_then(|_| run_witness_test_tx_test(
            "00144c9c3dfac4207d5d8cb89df5722cb3d712385e3f",
            &tx,
            &flags,
            1000,
            1
        ))
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_valid.json#L465
#[test]
fn witness_non_witness_single_anyone_hash_input_position_ok() {
    let tx = deserialize_hex::<Transaction>("01000000020001000000000000000000000000000000000000000000000000000000000000000000004847304402202a0b4b1294d70540235ae033d78e64b4897ec859c7b6f1b2b1d8a02e1d46006702201445e756d2254b0f1dfda9ab8e1e1bc26df9668077403204f32d16a49a36eb6983ffffffff00010000000000000000000000000000000000000000000000000000000000000100000049483045022100acb96cfdbda6dc94b489fd06f2d720983b5f350e31ba906cdbd800773e80b21c02200d74ea5bdf114212b4bbe9ed82c36d2e369e302dff57cb60d01c428f0bd3daab83ffffffff02e8030000000000000151e903000000000000015100000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        Ok(()),
        run_witness_test_tx_test(
            "2103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc71ac",
            &tx,
            &flags,
            1000,
            0
        )
        .and_then(|_| run_witness_test_tx_test(
            "2103596d3451025c19dbbdeb932d6bf8bfb4ad499b95b6f88db8899efac102e5fc71ac",
            &tx,
            &flags,
            1001,
            1
        ))
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_valid.json#L471
#[test]
fn witness_bip143_example1() {
    let tx = deserialize_hex::<Transaction>("01000000000102fe3dc9208094f3ffd12645477b3dc56f60ec4fa8e6f5d67c565d1c6b9216b36e000000004847304402200af4e47c9b9629dbecc21f73af989bdaa911f7e6f6c2e9394588a3aa68f81e9902204f3fcf6ade7e5abb1295b6774c8e0abd94ae62217367096bc02ee5e435b67da201ffffffff0815cf020f013ed6cf91d29f4202e8a58726b1ac6c79da47c23d1bee0a6925f80000000000ffffffff0100f2052a010000001976a914a30741f8145e5acadf23f751864167f32e0963f788ac000347304402200de66acf4527789bfda55fc5459e214fa6083f936b430a762c629656216805ac0220396f550692cd347171cbc1ef1f51e15282e837bb2b30860dc77c8f78bc8501e503473044022027dc95ad6b740fe5129e7e62a75dd00f291a2aeb1200b84b09d9e3789406b6c002201a9ecd315dd6a0e632ab20bbb98948bc0c6fb204f2c286963bb48517a7058e27034721026dccc749adc2a9d0d89497ac511f760f45c47dc5ed9cf352a58ac706453880aeadab210255a9626aebf5e29c0e6538428ba0d1dcf6ca98ffdf086aa8ced5e0d0215ea465ac00000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        Ok(()),
        run_witness_test_tx_test(
            "21036d5c20fa14fb2f635474c1dc4ef5909d4568e5569b79fc94d3448486e14685f8ac",
            &tx,
            &flags,
            156250000,
            0
        )
        .and_then(|_| run_witness_test_tx_test(
            "00205d1b56b63d714eebe542309525f484b7e9d6f686b3781b6f61ef925d66d6f6a0",
            &tx,
            &flags,
            4900000000,
            1
        ))
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_valid.json#L476
#[test]
fn witness_bip143_example2() {
    let tx = deserialize_hex::<Transaction>("01000000000102e9b542c5176808107ff1df906f46bb1f2583b16112b95ee5380665ba7fcfc0010000000000ffffffff80e68831516392fcd100d186b3c2c7b95c80b53c77e77c35ba03a66b429a2a1b0000000000ffffffff0280969800000000001976a914de4b231626ef508c9a74a8517e6783c0546d6b2888ac80969800000000001976a9146648a8cd4531e1ec47f35916de8e259237294d1e88ac02483045022100f6a10b8604e6dc910194b79ccfc93e1bc0ec7c03453caaa8987f7d6c3413566002206216229ede9b4d6ec2d325be245c5b508ff0339bf1794078e20bfe0babc7ffe683270063ab68210392972e2eb617b2388771abe27235fd5ac44af8e61693261550447a4c3e39da98ac024730440220032521802a76ad7bf74d0e2c218b72cf0cbc867066e2e53db905ba37f130397e02207709e2188ed7f08f4c952d9d13986da504502b8c3be59617e043552f506c46ff83275163ab68210392972e2eb617b2388771abe27235fd5ac44af8e61693261550447a4c3e39da98ac00000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        Ok(()),
        run_witness_test_tx_test(
            "0020ba468eea561b26301e4cf69fa34bde4ad60c81e70f059f045ca9a79931004a4d",
            &tx,
            &flags,
            16777215,
            0
        )
        .and_then(|_| run_witness_test_tx_test(
            "0020d9bbfbe56af7c4b7f960a70d7ea107156913d9e5a26b0a71429df5e097ca6537",
            &tx,
            &flags,
            16777215,
            1
        ))
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_valid.json#L481
#[test]
fn witness_bip143_example3() {
    let tx = deserialize_hex::<Transaction>("0100000000010280e68831516392fcd100d186b3c2c7b95c80b53c77e77c35ba03a66b429a2a1b0000000000ffffffffe9b542c5176808107ff1df906f46bb1f2583b16112b95ee5380665ba7fcfc0010000000000ffffffff0280969800000000001976a9146648a8cd4531e1ec47f35916de8e259237294d1e88ac80969800000000001976a914de4b231626ef508c9a74a8517e6783c0546d6b2888ac024730440220032521802a76ad7bf74d0e2c218b72cf0cbc867066e2e53db905ba37f130397e02207709e2188ed7f08f4c952d9d13986da504502b8c3be59617e043552f506c46ff83275163ab68210392972e2eb617b2388771abe27235fd5ac44af8e61693261550447a4c3e39da98ac02483045022100f6a10b8604e6dc910194b79ccfc93e1bc0ec7c03453caaa8987f7d6c3413566002206216229ede9b4d6ec2d325be245c5b508ff0339bf1794078e20bfe0babc7ffe683270063ab68210392972e2eb617b2388771abe27235fd5ac44af8e61693261550447a4c3e39da98ac00000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        Ok(()),
        run_witness_test_tx_test(
            "0020d9bbfbe56af7c4b7f960a70d7ea107156913d9e5a26b0a71429df5e097ca6537",
            &tx,
            &flags,
            16777215,
            0
        )
        .and_then(|_| run_witness_test_tx_test(
            "0020ba468eea561b26301e4cf69fa34bde4ad60c81e70f059f045ca9a79931004a4d",
            &tx,
            &flags,
            16777215,
            1
        ))
    );
}

// https://github.com/bitcoin/bitcoin/blob/7ee6c434ce8df9441abcf1718555cc7728a4c575/src/test/data/tx_valid.json#L486
#[test]
fn witness_bip143_example4() {
    let tx = deserialize_hex::<Transaction>("0100000000010136641869ca081e70f394c6948e8af409e18b619df2ed74aa106c1ca29787b96e0100000023220020a16b5755f7f6f96dbd65f5f0d6ab9418b89af4b1f14a1bb8a09062c35f0dcb54ffffffff0200e9a435000000001976a914389ffce9cd9ae88dcc0631e88a821ffdbe9bfe2688acc0832f05000000001976a9147480a33f950689af511e6e84c138dbbd3c3ee41588ac080047304402206ac44d672dac41f9b00e28f4df20c52eeb087207e8d758d76d92c6fab3b73e2b0220367750dbbe19290069cba53d096f44530e4f98acaa594810388cf7409a1870ce01473044022068c7946a43232757cbdf9176f009a928e1cd9a1a8c212f15c1e11ac9f2925d9002205b75f937ff2f9f3c1246e547e54f62e027f64eefa2695578cc6432cdabce271502473044022059ebf56d98010a932cf8ecfec54c48e6139ed6adb0728c09cbe1e4fa0915302e022007cd986c8fa870ff5d2b3a89139c9fe7e499259875357e20fcbb15571c76795403483045022100fbefd94bd0a488d50b79102b5dad4ab6ced30c4069f1eaa69a4b5a763414067e02203156c6a5c9cf88f91265f5a942e96213afae16d83321c8b31bb342142a14d16381483045022100a5263ea0553ba89221984bd7f0b13613db16e7a70c549a86de0cc0444141a407022005c360ef0ae5a5d4f9f2f87a56c1546cc8268cab08c73501d6b3be2e1e1a8a08824730440220525406a1482936d5a21888260dc165497a90a15669636d8edca6b9fe490d309c022032af0c646a34a44d1f4576bf6a4a74b67940f8faa84c7df9abe12a01a11e2b4783cf56210307b8ae49ac90a048e9b53357a2354b3334e9c8bee813ecb98e99a7e07e8c3ba32103b28f0c28bfab54554ae8c658ac5c3e0ce6e79ad336331f78c428dd43eea8449b21034b8113d703413d57761b8b9781957b8c0ac1dfe69f492580ca4195f50376ba4a21033400f6afecb833092a9a21cfdf1ed1376e58c5d1f47de74683123987e967a8f42103a6d48b1131e94ba04d9737d61acdaa1322008af9602b3b14862c07a1789aac162102d8b661b0b3302ee2f162b09e07a55ad5dfbe673a9f01d9f0c19617681024306b56ae00000000").unwrap();
    let flags = VerifyFlags::P2SH | VerifyFlags::WITNESS;
    assert_eq!(
        Ok(()),
        run_witness_test_tx_test(
            "a9149993a429037b5d912407a71c252019287b8d27a587",
            &tx,
            &flags,
            987654321,
            0
        )
    );
}
