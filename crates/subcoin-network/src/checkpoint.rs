use bitcoin::p2p::message::MAX_INV_SIZE;
use once_cell::sync::Lazy;
use subcoin_primitives::IndexedBlock;

// NOTE: The checkpoints were initially copied from btcd.
static CHECKPOINTS: Lazy<Vec<IndexedBlock>> = Lazy::new(|| {
    let mut start = 0u32;
    [
        (
            11111,
            "0000000069e244f73d78e8fd29ba2fd2ed618bd6fa2ee92559f542fdb26e7c1d",
        ),
        (
            33333,
            "000000002dd5588a74784eaa7ab0507a18ad16a236e7b1ce69f00d7ddfb5d0a6",
        ),
        (
            74000,
            "0000000000573993a3c9e41ce34471c079dcf5f52a0e824a81e7f953b8661a20",
        ),
        (
            105000,
            "00000000000291ce28027faea320c8d2b054b2e0fe44a773f3eefb151d6bdc97",
        ),
        (
            134444,
            "00000000000005b12ffd4cd315cd34ffd4a594f430ac814c91184a0d42d2b0fe",
        ),
        (
            168000,
            "000000000000099e61ea72015e79632f216fe6cb33d7899acb35b75c8303b763",
        ),
        (
            193000,
            "000000000000059f452a5f7340de6682a977387c17010ff6e6c3bd83ca8b1317",
        ),
        (
            210000,
            "000000000000048b95347e83192f69cf0366076336c639f9b7228e9ba171342e",
        ),
        (
            216116,
            "00000000000001b4f4b433e81ee46494af945cf96014816a4e2370f11b23df4e",
        ),
        (
            225430,
            "00000000000001c108384350f74090433e7fcf79a606b8e797f065b130575932",
        ),
        (
            250000,
            "000000000000003887df1f29024b06fc2200b55f8af8f35453d7be294df2d214",
        ),
        (
            267300,
            "000000000000000a83fbd660e918f218bf37edd92b748ad940483c7c116179ac",
        ),
        (
            279000,
            "0000000000000001ae8c72a0b0c301f67e3afca10e819efa9041e458e9bd7e40",
        ),
        (
            300255,
            "0000000000000000162804527c6e9b9f0563a280525f9d08c12041def0a0f3b2",
        ),
        (
            319400,
            "000000000000000021c6052e9becade189495d1c539aa37c58917305fd15f13b",
        ),
        (
            343185,
            "0000000000000000072b8bf361d01a6ba7d445dd024203fafc78768ed4368554",
        ),
        (
            352940,
            "000000000000000010755df42dba556bb72be6a32f3ce0b6941ce4430152c9ff",
        ),
        (
            382320,
            "00000000000000000a8dc6ed5b133d0eb2fd6af56203e4159789b092defd8ab2",
        ),
        (
            400000,
            "000000000000000004ec466ce4732fe6f1ed1cddc2ed4b328fff5224276e3f6f",
        ),
        (
            430000,
            "000000000000000001868b2bb3a285f3cc6b33ea234eb70facf4dcdf22186b87",
        ),
        (
            460000,
            "000000000000000000ef751bbce8e744ad303c47ece06c8d863e4d417efc258c",
        ),
        (
            490000,
            "000000000000000000de069137b17b8d5a3dfbd5b145b2dcfb203f15d0c4de90",
        ),
        (
            520000,
            "0000000000000000000d26984c0229c9f6962dc74db0a6d525f2f1640396f69c",
        ),
        (
            550000,
            "000000000000000000223b7a2298fb1c6c75fb0efc28a4c56853ff4112ec6bc9",
        ),
        (
            560000,
            "0000000000000000002c7b276daf6efb2b6aa68e2ce3be67ef925b3264ae7122",
        ),
        (
            563378,
            "0000000000000000000f1c54590ee18d15ec70e68c8cd4cfbadb1b4f11697eee",
        ),
        (
            597379,
            "00000000000000000005f8920febd3925f8272a6a71237563d78c2edfdd09ddf",
        ),
        (
            623950,
            "0000000000000000000f2adce67e49b0b6bdeb9de8b7c3d7e93b21e7fc1e819d",
        ),
        (
            654683,
            "0000000000000000000b9d2ec5a352ecba0592946514a92f14319dc2b367fc72",
        ),
        (
            691719,
            "00000000000000000008a89e854d57e5667df88f1cdef6fde2fbca1de5b639ad",
        ),
        (
            724466,
            "000000000000000000052d314a259755ca65944e68df6b12a067ea8f1f5a7091",
        ),
        (
            751565,
            "00000000000000000009c97098b5295f7e5f183ac811fb5d1534040adb93cabd",
        ),
        (
            781565,
            "00000000000000000002b8c04999434c33b8e033f11a977b288f8411766ee61c",
        ),
        (
            800000,
            "00000000000000000002a7c4c1e48d76c5a37902165a270156b7a8d72728a054",
        ),
        (
            810000,
            "000000000000000000028028ca82b6aa81ce789e4eb9e0321b74c3cbaf405dd1",
        ),
    ]
    .into_iter()
    .map(|(number, hash)| {
        // Ensure the headers downloaded according to the checkpoints can fit into one single `inv` message.
        if number - start > MAX_INV_SIZE as u32 {
            panic!("Range of checkpoints too wide");
        }

        start = number;

        IndexedBlock {
            number,
            hash: hash
                .parse()
                .expect("Checkpoint BlockHash must be valid; qed"),
        }
    })
    .collect()
});

pub(crate) fn next_checkpoint(block_number: u32) -> Option<IndexedBlock> {
    match CHECKPOINTS.binary_search_by_key(&block_number, |checkpoint| checkpoint.number) {
        Ok(_) => None,
        Err(index) => CHECKPOINTS.get(index).cloned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_next_checkpoint() {
        assert_eq!(next_checkpoint(800).unwrap().number, 11111);
    }
}
