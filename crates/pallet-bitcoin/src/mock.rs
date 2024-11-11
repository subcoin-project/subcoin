use crate as pallet_bitcoin;
use crate::{CoinsCount, Config};
use bitcoin::consensus::deserialize;
use frame_support::derive_impl;
use frame_support::traits::Everything;
use hex::test_hex_unwrap as hex;
use sp_runtime::BuildStorage;

type Block = frame_system::mocking::MockBlock<Test>;

frame_support::construct_runtime!(
    pub enum Test
    {
        System: frame_system,
        Bitcoin: pallet_bitcoin,
    }
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type BaseCallFilter = Everything;
    type Block = Block;
}

impl Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type WeightInfo = ();
}

// Build test environment by setting the root `key` for the Genesis.
pub fn new_test_ext() -> sp_io::TestExternalities {
    let mut t = frame_system::GenesisConfig::<Test>::default()
        .build_storage()
        .unwrap();
    crate::GenesisConfig::<Test>::default()
        .assimilate_storage(&mut t)
        .unwrap();
    let mut ext: sp_io::TestExternalities = t.into();
    ext.execute_with(|| System::set_block_number(1));
    ext
}

#[test]
fn test_coins_count() {
    new_test_ext().execute_with(|| {
        let tx_bytes = hex!(
            "010000000001010000000000000000000000000000000000000000000000000000000000000000ffffffff4f03d5450d122f4d617869506f6f6c2f25012803abe54653fabe6d6d4ad45e9784a830a2cb8354e836f855bc59fb44cb30f01ca95167920426208c801000000000000000000010080697000000000000ffffffff041f784e130000000017a914388dbac6cfab015de49b65de1f10ba179c930e73870000000000000000266a24aa21a9ed2584622ee5260f69459ddc0ded8914745ac20bc39b6a2e46a9beb1b61da2c75300000000000000002f6a2d434f52450142fdeae88682a965939fee9b7b2bd5b99694ff64e7a2e476a0bf36a4d7ce240d01e38764eba9685c00000000000000002b6a2952534b424c4f434b3a7cefdb7cd36978cd3708c75a0d35fd93ddfe74695e327fb0a841d312006933fa0120000000000000000000000000000000000000000000000000000000000000000000000000"
        );
        let tx: bitcoin::Transaction = deserialize(&tx_bytes).unwrap();
        let tx: crate::types::Transaction = tx.into();

        assert_eq!(CoinsCount::<Test>::get(), 0);
        Bitcoin::transact(RuntimeOrigin::none(), tx).unwrap();
        assert_eq!(CoinsCount::<Test>::get(), 4);
    });
}
