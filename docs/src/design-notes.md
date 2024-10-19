# Design Notes

## Storage

By default, Subcoin simply stores Bitcoin data in their original byte order (little-endian). However, displaying Bitcoin block hashes and transaction IDs (txids) in this format is not user-friendly, as the broader Bitcoin ecosystem presents these values in big-endian order. To improve readability on platforms like polkadot.js.org, we will store certain Bitcoin data in big-endian format whenever it needs to be displayed.

The following data will now be stored in big-endian format to align with Bitcoin ecosystem display conventions:

- `txid` in `pallet_bitcoin::Call::transact { btc_tx: Transaction }`.
- `hash` in the digest of the block header.
