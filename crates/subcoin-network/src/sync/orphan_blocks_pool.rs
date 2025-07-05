#![allow(dead_code)]

use bitcoin::{Block as BitcoinBlock, BlockHash};
use std::collections::{HashMap, HashSet, VecDeque};

/// Storage for the blocks without parent yet.
///
/// Blocks from this storage are either moved to verification queue, or removed at all.
#[derive(Debug, Clone)]
pub struct OrphanBlocksPool {
    /// Mapping of the block hash to the full block data.
    blocks: HashMap<BlockHash, BitcoinBlock>,
    /// Blocks with unknown parents. They may be block announcement or received out-of-order.
    ///
    /// block_hash => Vec<child_block_hash>
    ///
    /// A block could have multiple children blocks.
    orphan_blocks: HashMap<BlockHash, HashSet<BlockHash>>,
    /// Blocks that we have received but we didn't ask for.
    unknown_blocks: HashSet<BlockHash>,
}

impl OrphanBlocksPool {
    /// Constructs a new [`OrphanBlocksPool`].
    pub(crate) fn new() -> Self {
        Self {
            blocks: HashMap::new(),
            orphan_blocks: HashMap::new(),
            unknown_blocks: HashSet::new(),
        }
    }

    /// Returns the total number of orphan blocks in pool.
    pub(crate) fn len(&self) -> usize {
        self.blocks.len()
    }

    /// Returns `true` if the block specified by `hash` already exists in the pool.
    pub(crate) fn block_exists(&self, hash: &BlockHash) -> bool {
        self.blocks.contains_key(hash)
    }

    /// Returns `true` if the block with given hash is stored as unknown in this pool.
    pub(crate) fn contains_unknown_block(&self, hash: &BlockHash) -> bool {
        self.unknown_blocks.contains(hash)
    }

    /// Returns `true` if the block with given parent hash is stored in this pool.
    pub(crate) fn contains_orphan_block(&self, parent_hash: &BlockHash) -> bool {
        self.orphan_blocks.contains_key(parent_hash)
    }

    /// Returns all the unknown blocks in the insertion order.
    pub(crate) fn unknown_blocks(&self) -> &HashSet<BlockHash> {
        &self.unknown_blocks
    }

    pub(crate) fn clear(&mut self) {
        self.blocks.clear();
        self.orphan_blocks.clear();
        self.unknown_blocks.clear();
    }

    /// Insert orphaned block, for which we have already requested its parent block
    pub(crate) fn insert_orphan_block(&mut self, block: BitcoinBlock) {
        let block_hash = block.block_hash();
        self.orphan_blocks
            .entry(block.header.prev_blockhash)
            .or_default()
            .insert(block_hash);
        self.blocks.insert(block_hash, block);
    }

    /// Insert unknown block, for which we know nothing about its parent block
    pub(crate) fn insert_unknown_block(&mut self, block: BitcoinBlock) {
        let newly_inserted = self.unknown_blocks.insert(block.block_hash());

        assert!(newly_inserted);

        self.insert_orphan_block(block);
    }

    /// Remove all blocks, which are not-unknown
    pub(crate) fn remove_known_blocks(&mut self) -> Vec<BlockHash> {
        let orphans_to_remove: HashSet<_> = self
            .orphan_blocks
            .values()
            .flatten()
            .cloned()
            .filter(|h| !self.unknown_blocks.contains(h))
            .collect();
        self.remove_blocks(&orphans_to_remove);
        orphans_to_remove.into_iter().collect()
    }

    /// Remove all blocks whose ancestor is `hash`.
    pub(crate) fn remove_blocks_for_parent(&mut self, hash: BlockHash) -> VecDeque<BitcoinBlock> {
        let mut queue: VecDeque<BlockHash> = VecDeque::new();
        queue.push_back(hash);

        let mut removed: VecDeque<BitcoinBlock> = VecDeque::new();

        while let Some(parent_hash) = queue.pop_front() {
            if let Some(children) = self.orphan_blocks.remove(&parent_hash) {
                for child_hash in children {
                    self.unknown_blocks.remove(&child_hash);

                    queue.push_back(child_hash);

                    if let Some(block) = self.blocks.remove(&child_hash) {
                        removed.push_back(block);
                    }
                }
            }
        }

        removed
    }

    /// Remove blocks with given hashes + all dependent blocks
    pub(crate) fn remove_blocks(&mut self, hashes: &HashSet<BlockHash>) -> Vec<BlockHash> {
        let mut removed = Vec::new();

        self.orphan_blocks.retain(|_, orphans| {
            for hash in hashes {
                if orphans.remove(hash) {
                    removed.push(*hash);
                }
            }
            !orphans.is_empty()
        });

        removed.iter().for_each(|block_hash| {
            self.unknown_blocks.remove(block_hash);
            self.blocks.remove(block_hash);
        });

        // also delete all children
        for hash in hashes.iter() {
            removed.extend(
                self.remove_blocks_for_parent(*hash)
                    .iter()
                    .map(|block| block.block_hash()),
            );
        }

        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::consensus::Decodable;
    use bitcoin::hex::FromHex;
    use std::collections::HashSet;

    fn decode_block(block_in_hex: &str) -> BitcoinBlock {
        let data = Vec::<u8>::from_hex(block_in_hex).expect("Failed to convert hex str");
        bitcoin::Block::consensus_decode(&mut data.as_slice()).expect("Bad block in the database")
    }

    fn block0() -> BitcoinBlock {
        decode_block(
            "0100000000000000000000000000000000000000000000000000000000000000000000003ba3edfd7a7b12b27ac72c3e67768f617fc81bc3888a51323a9fb8aa4b1e5e4a29ab5f49ffff001d1dac2b7c0101000000010000000000000000000000000000000000000000000000000000000000000000ffffffff4d04ffff001d0104455468652054696d65732030332f4a616e2f32303039204368616e63656c6c6f72206f6e206272696e6b206f66207365636f6e64206261696c6f757420666f722062616e6b73ffffffff0100f2052a01000000434104678afdb0fe5548271967f1a67130b7105cd6a828e03909a67962e0ea1f61deb649f6bc3f4cef38c4f35504e51ec112de5c384df7ba0b8d578a4c702b6bf11d5fac00000000",
        )
    }

    fn block1() -> BitcoinBlock {
        decode_block(
            "010000006fe28c0ab6f1b372c1a6a246ae63f74f931e8365e15a089c68d6190000000000982051fd1e4ba744bbbe680e1fee14677ba1a3c3540bf7b1cdb606e857233e0e61bc6649ffff001d01e362990101000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0704ffff001d0104ffffffff0100f2052a0100000043410496b538e853519c726a2c91e61ec11600ae1390813a627c66fb8be7947be63c52da7589379515d4e0a604f8141781e62294721166bf621e73a82cbf2342c858eeac00000000",
        )
    }
    fn block2() -> BitcoinBlock {
        decode_block(
            "010000004860eb18bf1b1620e37e9490fc8a427514416fd75159ab86688e9a8300000000d5fdcc541e25de1c7a5addedf24858b8bb665c9f36ef744ee42c316022c90f9bb0bc6649ffff001d08d2bd610101000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0704ffff001d010bffffffff0100f2052a010000004341047211a824f55b505228e4c3d5194c1fcfaa15a456abdf37f9b9d97a4040afc073dee6c89064984f03385237d92167c13e236446b417ab79a0fcae412ae3316b77ac00000000",
        )
    }
    fn block3() -> BitcoinBlock {
        decode_block(
            "01000000bddd99ccfda39da1b108ce1a5d70038d0a967bacb68b6b63065f626a0000000044f672226090d85db9a9f2fbfe5f0f9609b387af7be5b7fbb7a1767c831c9e995dbe6649ffff001d05e0ed6d0101000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0704ffff001d010effffffff0100f2052a0100000043410494b9d3e76c5b1629ecf97fff95d7a4bbdac87cc26099ada28066c6ff1eb9191223cd897194a08d0c2726c5747f1db49e8cf90e75dc3e3550ae9b30086f3cd5aaac00000000",
        )
    }

    fn block169() -> BitcoinBlock {
        decode_block(
            "01000000696aa63f0f22d9189c8536bb83b18737ae8336c25a67937f79957e5600000000982db9870a5e30d8f0b2a4ebccc5852b5a1e2413e9274c4947bfec6bdaa9b9d75bb76a49ffff001d2b719fdd0101000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0704ffff001d0101ffffffff0100f2052a010000004341045da87c7b825c75ca17ade8bb5cdbcd27af4ce97373aa9848c0c84693ca857cf379e14c2ce61ea2aaee9450d0939e21bd26894aa6dcc808656fa9974dc296589eac00000000",
        )
    }

    fn block170() -> BitcoinBlock {
        decode_block(
            "0100000055bd840a78798ad0da853f68974f3d183e2bd1db6a842c1feecf222a00000000ff104ccb05421ab93e63f8c3ce5c2c2e9dbb37de2764b3a3175c8166562cac7d51b96a49ffff001d283e9e700201000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0704ffff001d0102ffffffff0100f2052a01000000434104d46c4968bde02899d2aa0963367c7a6ce34eec332b32e42e5f3407e052d64ac625da6f0718e7b302140434bd725706957c092db53805b821a85b23a7ac61725bac000000000100000001c997a5e56e104102fa209c6a852dd90660a20b2d9c352423edce25857fcd3704000000004847304402204e45e16932b8af514961a1d3a1a25fdf3f4f7732e9d624c6c61548ab5fb8cd410220181522ec8eca07de4860a4acdd12909d831cc56cbbac4622082221a8768d1d0901ffffffff0200ca9a3b00000000434104ae1a62fe09c5f51b13905f07f06b99a2f7159b2225f374cd378d71302fa28414e7aab37397f554a7df5f142c21c1b7303b8a0626f1baded5c72a704f7e6cd84cac00286bee0000000043410411db93e1dcdb8a016b49840f8c53bc1eb68a382e97b1482ecad7b148a6909a5cb2e0eaddfb84ccf9744464f82e160bfa9b8b64f9d4c03f999b8643f656b412a3ac00000000",
        )
    }

    fn block181() -> BitcoinBlock {
        decode_block(
            "01000000f2c8a8d2af43a9cd05142654e56f41d159ce0274d9cabe15a20eefb500000000366c2a0915f05db4b450c050ce7165acd55f823fee51430a8c993e0bdbb192ede5dc6a49ffff001d192d3f2f0201000000010000000000000000000000000000000000000000000000000000000000000000ffffffff0704ffff001d0128ffffffff0100f2052a0100000043410435f0d8366085f73906a48309728155532f24293ea59fe0b33a245c4b8d75f82c3e70804457b7f49322aa822196a7521e4931f809d7e489bccb4ff14758d170e5ac000000000100000001169e1e83e930853391bc6f35f605c6754cfead57cf8387639d3b4096c54f18f40100000048473044022027542a94d6646c51240f23a76d33088d3dd8815b25e9ea18cac67d1171a3212e02203baf203c6e7b80ebd3e588628466ea28be572fe1aaa3f30947da4763dd3b3d2b01ffffffff0200ca9a3b00000000434104b5abd412d4341b45056d3e376cd446eca43fa871b51961330deebd84423e740daa520690e1d9e074654c59ff87b408db903649623e86f1ca5412786f61ade2bfac005ed0b20000000043410411db93e1dcdb8a016b49840f8c53bc1eb68a382e97b1482ecad7b148a6909a5cb2e0eaddfb84ccf9744464f82e160bfa9b8b64f9d4c03f999b8643f656b412a3ac00000000",
        )
    }

    #[test]
    fn orphan_block_pool_insert_orphan_block() {
        let mut pool = OrphanBlocksPool::new();
        let b1 = block1();
        let b1_hash = b1.block_hash();

        pool.insert_orphan_block(b1);

        assert_eq!(pool.len(), 1);
        assert!(!pool.contains_unknown_block(&b1_hash));
        assert_eq!(pool.unknown_blocks().len(), 0);
    }

    #[test]
    fn orphan_block_pool_insert_unknown_block() {
        let mut pool = OrphanBlocksPool::new();
        let b1 = block1();
        let b1_hash = b1.block_hash();

        pool.insert_unknown_block(b1.into());

        assert_eq!(pool.len(), 1);
        assert!(pool.contains_unknown_block(&b1_hash));
        assert_eq!(pool.unknown_blocks().len(), 1);
    }

    #[test]
    fn orphan_block_pool_remove_known_blocks() {
        let mut pool = OrphanBlocksPool::new();
        let b1 = block1();
        let b1_hash = b1.block_hash();
        let b2 = block169();
        let b2_hash = b2.block_hash();

        pool.insert_orphan_block(b1);
        pool.insert_unknown_block(b2);

        assert_eq!(pool.len(), 2);
        assert!(!pool.contains_unknown_block(&b1_hash));
        assert!(pool.contains_unknown_block(&b2_hash));
        assert_eq!(pool.unknown_blocks().len(), 1);

        pool.remove_known_blocks();

        assert_eq!(pool.len(), 1);
        assert!(!pool.contains_unknown_block(&b1_hash));
        assert!(pool.contains_unknown_block(&b2_hash));
        assert_eq!(pool.unknown_blocks().len(), 1);
    }

    #[test]
    fn orphan_block_pool_remove_blocks_for_parent() {
        let mut pool = OrphanBlocksPool::new();
        let b0_hash = block0().block_hash();
        let b1 = block1();
        let b1_hash = b1.block_hash();
        let b2 = block169();
        let b2_hash = b2.block_hash();
        let b3 = block2();

        pool.insert_orphan_block(b1.clone());
        pool.insert_unknown_block(b2);
        pool.insert_orphan_block(b3.clone());

        let removed: Vec<_> = pool.remove_blocks_for_parent(b0_hash).into();
        assert_eq!(removed, vec![b1, b3]);

        assert_eq!(pool.len(), 1);
        assert_eq!(pool.unknown_blocks().len(), 1);
        assert!(!pool.contains_unknown_block(&b1_hash));
        assert!(pool.contains_unknown_block(&b2_hash));
        assert!(!pool.contains_unknown_block(&b1_hash));
    }

    #[test]
    fn orphan_block_pool_remove_blocks() {
        let mut pool = OrphanBlocksPool::new();
        let b1 = block1();
        let b1_hash = b1.block_hash();
        let b2 = block2();
        let b2_hash = b2.block_hash();
        let b3 = block169();
        let b3_hash = b3.block_hash();
        let b4 = block170();
        let b4_hash = b4.block_hash();
        let b5 = block181();

        pool.insert_orphan_block(b1);
        pool.insert_orphan_block(b2);
        pool.insert_orphan_block(b3);
        pool.insert_orphan_block(b4);
        pool.insert_orphan_block(b5);

        let blocks_to_remove: HashSet<BlockHash> = HashSet::from_iter([b1_hash, b3_hash]);

        let removed = pool.remove_blocks(&blocks_to_remove);
        assert_eq!(
            removed.into_iter().collect::<HashSet<_>>(),
            HashSet::from_iter([b1_hash, b2_hash, b3_hash, b4_hash])
        );

        assert_eq!(pool.len(), 1);
    }
}
