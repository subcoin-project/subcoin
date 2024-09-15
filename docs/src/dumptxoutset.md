# Export Bitcoin State Compatible with Bitcoin Core

## `dumptxoutset` Command

Subcoin provides the ability to export the Bitcoin state (UTXO Set) into a binary file that is fully compatible with the output of Bitcoin Core's `dumptxoutset` command, allowing for seamless integration and compatibility with Bitcoin Core.

To use this feature, check out this command:

```bash
subcoin blockchain dumptxoutset --help
```

## Decentralized UTXO Set Snapshot Download (Coming Soon)

You can download the Bitcoin state directly from the Subcoin P2P network in a fully decentralized manner, eliminating the need for trusted snapshot providers. This feature will allow you to retrieve the UTXO Set snapshot from the network, convert it into a format compatible with Bitcoin Core, and import it directly into a Bitcoin Core node. This makes Subcoin an ideal addition to the Bitcoin Core ecosystem, providing a decentralized, secure, and efficient method for syncing with Bitcoin’s UTXO set.

Follow [Subcoin Issue #54](https://github.com/subcoin-project/subcoin/issues/54) for this complementary feature, which will enhance the functionality of Bitcoin Core by integrating decentralized state retrieval from Subcoin.
