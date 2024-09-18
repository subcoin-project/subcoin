# Subcoin

Subcoin is Bitcoin full node in Rust implemented in the [Substrate framework](https://github.com/paritytech/polkadot-sdk).

Subcoin's most significant contribution is introducing decentralized fast sync to
the Bitcoin ecosystem. Unlike traditional initial full sync, which require processing
every historical block and replaying all past transactions, fast sync downloads
the state at a specific block and begins normal block sync from that point.

## Resources

- [Subcoin: A Step Toward Decentralized Fast Sync for Bitcoin](https://www.notion.so/liuchengxu/Subcoin-A-Step-Toward-Decentralized-Fast-Sync-for-Bitcoin-68762427a4484d73906a91602d789be9)
