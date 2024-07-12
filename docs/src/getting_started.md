# Getting Started

## Installation

### Compile from source

#### Install prerequisites

Since subcoin is a Substrate-based chain, you must install the necessary development tools to compile the binary from source.
Please refer to https://docs.substrate.io/install/ for the full guide of installing the prerequisites.

#### Compile subcoin node binary

Once the required packages and Rust installed, we can proceed to compile the binary.

```bash
cargo build --profile production --bin subcoin
```

The Subcoin node executable `subcoin` should be present at `target/production/subcoin`.

### Docker

```
```

## Import bitcoin blocks from `bitcoind` database

### Run `bitcoind`

Firstly, we need to install the `bitcoind` binary which can be downloaded directly from [https://bitcoincore.org/en/download](https://bitcoincore.org/en/download/).
And then we need to spin up a `bitcoind` node with `txindex` and `coinstatsindex` enabled. `txindex` is required to import the blocks in subcoin, `coinstatsindex` is required
to query the UTXO set of specific block later.

For instance, we use `/tmp/btc-data` as the data dir:

<!-- TODO: specify the exact version of bitcoind we are using here. -->

```bash
mkdir -p /tmp/btc-data && ./src/bitcoind -datadir=/tmp/btc-data -txindex -coinstatsindex
```

Keep the `bitcoind` process running for a while and ensure it has synced a number of blocks.

```
...
8-29T01:17:02Z' progress=0.001342 cache=33.0MiB(304329txo)
2024-07-11T17:23:10Z UpdateTip: new best=00000000000008c273c4c215892eacbafec33c199cfd3d9b539cdb6aafc39f54 height=142979 version=0x00000001 log2_work=66.385350 tx=1392173 date='2011-08-29T01:23:19Z' progress=0.001342 cache=33.0MiB(304442txo)
2024-07-11T17:23:10Z UpdateTip: new best=000000000000000f338e8635c5f78666f0ca2e6b70425fe22aad47d2087a2740 height=142980 version=0x00000001 log2_work=66.385466 tx=1392186 date='2011-08-29T01:29:29Z' progress=0.001342 cache=33.0MiB(304451txo)
2024-07-11T17:23:10Z UpdateTip: new best=000000000000071504dedbc2edd4ae008aca9a6086afb3d4d9cd3cbdd0e67b04 height=142981 version=0x00000001 log2_work=66.385582 tx=1392218 date='2011-08-29T01:35:02Z' progress=0.001342 cache=33.0MiB(304470txo)
2024-07-11T17:23:10Z UpdateTip: new best=00000000000001c59463d1a0b6d70274ed4aea4cf757289363ff99f670f02812 height=142982 version=0x00000001 log2_work=66.385698 tx=1392232 date='2011-08-29T01:37:36Z' progress=0.001342 cache=33.0MiB(304605txo)
```

Now stop the `bitcoind` process and proceed to import the blocks in `bitcoind` database into subcoin.

### Run `subcoin import-blocks`

```bash
target/release/subcoin import-blocks /tmp/btc-data
```

You'll see the output like this:

```
2024-07-12 01:28:51 🔨 Initializing Genesis block/state (state: 0x1c68…f3f8, header-hash: 0xbdc8…a76b)
2024-07-12 01:28:53 🏁 CPU score: 1.15 GiBs
2024-07-12 01:28:53 🏁 Memory score: 13.87 GiBs
2024-07-12 01:28:53 🏁 Disk score (seq. writes): 1.77 GiBs
2024-07-12 01:28:53 🏁 Disk score (rand. writes): 678.77 MiBs
2024-07-12 01:28:53 Start loading block_index
2024-07-12 01:28:53 Successfully opened tx_index DB!
2024-07-12 01:28:53 Start to import blocks from #1 to #142984 from bitcoind database: /tmp/btc-data
2024-07-12 01:28:54 Imported 1000 blocks,, best#1001,00000000a2887344f8db859e372e7e4bc26b23b9de340f725afbf2edb265b4c6 (0x3ff4…6eb5)
2024-07-12 01:28:54 Imported 2000 blocks, 3802.2 bps, best#2001,0000000067217a46c49054bad67cda2da943607d326e89896786de10b07cb7c0 (0x296d…6a47)
2024-07-12 01:28:54 Imported 3000 blocks, 3636.3 bps, best#3001,00000000ee1d6b98d28b71c969d4bc8a20ee43a379ce49547bcad30c606d8845 (0xac16…c220)
2024-07-12 01:28:54 Imported 4000 blocks, 3597.1 bps, best#4001,00000000a86f68e8de06c6b46623fdd16b7a11ad9651fa48ecbe8c731658dc06 (0xb5bd…8a28)
2024-07-12 01:28:55 Imported 5000 blocks, 3773.5 bps, best#5001,00000000284bcd658fd7a76f5a88ee526f18592251341a05fd7f3d7abaf0c3ec (0x751f…3375)
2024-07-12 01:28:55 Imported 6000 blocks, 3484.3 bps, best#6001,0000000055fcaf04cb9a82bb86b46a21b15fcaa75ac8c18679b0234f79c4c615 (0xcc4a…3090)
...
```

Note that the `bitcoind` process must be stopped before running `subcoin import-blocks` otherwise you will run into the error as following.

```
Error: Application(OpError { kind: None, message: "LevelDB error: IO error: lock /tmp/btc-data/blocks/index/LOCK: Resource temporarily unavailable" })
```

### Verify the state of UTXO set

Once the bitcoin blocks are imported to subcoin node successfully, we can check the correctness of the UTXO set.

```bash
./src/bitcoin-cli -datadir=/tmp/btc-data gettxoutsetinfo none 10000 true
```

```bash
./target/release/subcoin blockchain gettxoutsetinfo --height 10000 -d /tmp/subcoin-data
```
