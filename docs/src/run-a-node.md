# Run Subcoin Node

## System Requirements

TODO

## Syncing the Bitcoin Network

Run the following command to sync the Bitcoin blockchain from the Bitcoin P2P network. The `--log subcoin_network=debug` option
will enable debug-level logging to show detailed information about the syncing process. By default, the block will be fully verified.
You can use `--block-verification=none` to skip the block verification. Check out `subcoin run --help` for more options.

```bash
subcoin run -d data --log subcoin_network=debug
```
