# Subcoin

Subcoin is Bitcoin full node in Rust implemented in the [Substrate framework](https://github.com/paritytech/polkadot-sdk).

Subcoin's most significant contribution is introducing decentralized fast sync to
the Bitcoin ecosystem. Unlike traditional initial full sync, which require processing
every historical block and replaying all past transactions, fast sync downloads
the state at a specific block and begins normal block sync from that point. Fast sync
is common and often the default sync strategy in newer blockchains with a global state
like Ethereum, Near, and Polkadot.
