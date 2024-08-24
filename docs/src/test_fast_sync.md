# Subcoin Fast Sync

> [!NOTE]
>
> The initial goal was to enable new Subcoin nodes to catch up with the latest Bitcoin state using fast sync.
> However, due to limitations in the current Substrate fast sync implementation, it's not feasible to sync to
> the tip of the Bitcoin network when the state exceeds several GiBs. This page demonstrates that the fast sync
> feature works well when the state is smaller. Solutions for handling large state syncs are being explored and
> will be supported in future updates.

To demonstrate the fast sync feature in Subcoin, we need a subcoin archive node that has synced a number of blocks from Bitcoin P2P network, and then start a new node to sync from the archive node using the Substrate fast sync mechanism. Once the fast sync from the Substrate networking is finished, the fast sync node can continue syncing blocks from Bitcoin network, we can confirm the fast sync works.

## Local Testing

Follow these steps to demonstrate the fast sync feature in Subcoin on a local setup:

### 1. Start an Archive Node

First, start an archive node that will sync a number of blocks from the Bitcoin P2P network. Use the following command:

```bash
rm -rf archive-data && ./target/release/subcoin run -d archive-data --log subcoin_network=debug --state-pruning=archive
```

### 2. Restart the Archive Node with Additional Flags 

After the archive node has synced some blocks, stop it and then restart it with the following flags:

- `--disable-subcoin-networking`: Disables Subcoin networking, allowing the node to operate with only Substrate networking. This ensures that once the fast sync node completes the fast sync and begins syncing Subcoin blocks, the archive node can receive blocks from the fast sync node via Substrate networking.
- `--allow-private-ip`: Enables Substrate networking to accept connections from other local Subcoin nodes.

Stop the archive node and use this command to restart it:

```bash
# Restart the archive node.
./target/release/subcoin run -d archive-data --allow-private-ip --log sync=debug --disable-subcoin-networking --state-pruning=archive
2024-08-24 23:48:17.821  INFO main sc_sysinfo: 💻 CPU architecture: x86_64    
2024-08-24 23:48:17.821  INFO main sc_sysinfo: 💻 Target environment: gnu    
2024-08-24 23:48:17.821  INFO main sc_sysinfo: 💻 CPU: AMD Ryzen 9 5900X 12-Core Processor    
2024-08-24 23:48:17.821  INFO main sc_sysinfo: 💻 CPU cores: 12    
2024-08-24 23:48:17.821  INFO main sc_sysinfo: 💻 Memory: 128751MB    
2024-08-24 23:48:17.821  INFO main sc_sysinfo: 💻 Kernel: 6.8.0-40-generic    
2024-08-24 23:48:17.821  INFO main sc_sysinfo: 💻 Linux distribution: Ubuntu 22.04.4 LTS    
2024-08-24 23:48:17.821  INFO main sc_sysinfo: 💻 Virtual machine: no    
2024-08-24 23:48:17.821  INFO tokio-runtime-worker prometheus: 〽️ Prometheus exporter started at 127.0.0.1:9615    
2024-08-24 23:48:17.821  INFO                 main sc_rpc_server: Running JSON-RPC server: addr=127.0.0.1:9944, allowed origins=[]    
2024-08-24 23:48:22.822  INFO tokio-runtime-worker substrate: 💤 Idle (0 peers), best: #67321 (0xd2ed…9e26), finalized #67315 (0x87b1…9f3d), ⬇ 0 ⬆ 0    
2024-08-24 23:48:27.822  INFO tokio-runtime-worker substrate: 💤 Idle (0 peers), best: #67321 (0xd2ed…9e26), finalized #67315 (0x87b1…9f3d), ⬇ 0 ⬆ 0    
2024-08-24 23:48:32.822  INFO tokio-runtime-worker substrate: 💤 Idle (0 peers), best: #67321 (0xd2ed…9e26), finalized #67315 (0x87b1…9f3d), ⬇ 0 ⬆ 0    
2024-08-24 23:48:37.823  INFO tokio-runtime-worker substrate: 💤 Idle (0 peers), best: #67321 (0xd2ed…9e26), finalized #67315 (0x87b1…9f3d), ⬇ 0 ⬆ 0    
2024-08-24 23:48:42.823  INFO tokio-runtime-worker substrate: 💤 Idle (0 peers), best: #67321 (0xd2ed…9e26), finalized #67315 (0x87b1…9f3d), ⬇ 0 ⬆ 0    
```

### 3. Start a New Node for Fast Sync

Now, start a new node that will perform a fast sync from the archive node using the Substrate fast sync mechanism:

```bash
./target/release/subcoin run -d fast-sync-node-data --sync fast-unsafe --state-pruning=256 --blocks-pruning=256 --log subcoin_network=debug,sync=debug --port 30334
...
2024-08-24 23:51:16.537  INFO tokio-runtime-worker substrate: 💤 Idle (1 peers), best: #67319 (0xe412…f0b1), finalized #49286 (0x1c72…9082), ⬇ 6.5MiB/s ⬆ 23.6kiB/s
2024-08-24 23:51:16.565  INFO tokio-runtime-worker subcoin: 💤 Idle (0 peers), best: #67319 (0000…835512,0xe412…f0b1), finalized #49286 (0000…b7f35a,0x1c72…9082), ⬇ 0 ⬆ 0
2024-08-24 23:51:16.576  INFO tokio-runtime-worker subcoin_service::finalizer: ✅ Successfully finalized block #67312,0xb409…11ef
2024-08-24 23:51:16.577  INFO tokio-runtime-worker subcoin_service::finalizer: ✅ Successfully finalized block #67314,0xc59a…7aa0
2024-08-24 23:51:16.577  INFO tokio-runtime-worker subcoin_service::finalizer: ✅ Successfully finalized block #67315,0x87b1…9f3d
2024-08-24 23:51:16.577 DEBUG tokio-runtime-worker sync: Starting state sync for #67314 (0xc59a…7aa0)
2024-08-24 23:51:16.653 DEBUG tokio-runtime-worker sync: Importing state data from 12D3KooWEZDBxWVrbSP8trLvk5Yoi2ZjErtkzDJ636M4cs3RXdy6 with 1 keys, 0 proof nodes.
2024-08-24 23:51:16.653 DEBUG tokio-runtime-worker sync: Importing state from Some(be89f1f86dcd96a26d6c3308a396e3812149f4e1bd7e9f4e1c267e017c117d87397a77d4643b213cf5980c80a289384f79abd73908f9e0d235f16d3c1e0a0b5e00000000) to Some(26aa394eea5630e07c48ae0c9558cef702a5c1b19ab7a04f536c519aca4983ac)
2024-08-24 23:51:16.747 DEBUG tokio-runtime-worker sync: Importing state data from 12D3KooWEZDBxWVrbSP8trLvk5Yoi2ZjErtkzDJ636M4cs3RXdy6 with 1 keys, 0 proof nodes.
2024-08-24 23:51:16.747 DEBUG tokio-runtime-worker sync: Importing state from Some(be89f1f86dcd96a26d6c3308a396e3812149f4e1bd7e9f4e1c267e017c117d877b0425e436196154e06a70737edd23593b0cfcfedaff737ad85b016d18e8531800000000) to Some(be89f1f86dcd96a26d6c3308a396e3812149f4e1bd7e9f4e1c267e017c117d87397a9be1ebc8aec938565deba17bbd79dcdf32a5a89a501ed874e8d11e6a5ea700000000)
2024-08-24 23:51:16.775 DEBUG tokio-runtime-worker subcoin_network::connection: New connection peer_addr=34.106.78.170:8333 local_addr=192.168.31.180:60556 direction=Outbound connect_latency=209
2024-08-24 23:51:16.790 DEBUG tokio-runtime-worker subcoin_network::connection: New connection peer_addr=134.209.215.174:8333 local_addr=192.168.31.180:40208 direction=Outbound connect_latency=225
2024-08-24 23:51:16.803 DEBUG tokio-runtime-worker subcoin_network::connection: New connection peer_addr=148.113.159.109:8333 local_addr=192.168.31.180:37222 direction=Outbound connect_latency=237
```

### 4. Monitor the Fast Sync Process

From the logs of the fast sync node, you should observe that the state sync is performed first, followed by the normal Subcoin block sync:

``` 
2024-08-24 23:51:16.577 DEBUG tokio-runtime-worker sync: Starting state sync for #67314 (0xc59a…7aa0)
2024-08-24 23:51:16.894 DEBUG tokio-runtime-worker sync: State download is complete. Import is queued
2024-08-24 23:51:17.337  INFO tokio-runtime-worker subcoin_service: Detected state sync is complete, starting Subcoin block sync
```

The fast sync node will continue syncing from block height #67321. The archive node will also continue to grow as the fast sync node keeps syncing, indicating that the Substrate syncing process is functioning correctly.

```
2024-08-24 23:51:02.834  INFO tokio-runtime-worker substrate: 💤 Idle (0 peers), best: #67321 (0xd2ed…9e26), finalized #67315 (0x87b1…9f3d), ⬇ 0 ⬆ 0
2024-08-24 23:51:07.835  INFO tokio-runtime-worker substrate: 💤 Idle (0 peers), best: #67321 (0xd2ed…9e26), finalized #67315 (0x87b1…9f3d), ⬇ 0 ⬆ 0
2024-08-24 23:51:12.835  INFO tokio-runtime-worker libp2p_mdns::behaviour: discovered: 12D3KooWSHAYsRX8zqb9Jkm29Z4hdhqt8Z8fazQS3iD3wKaJQBWS /ip4/172.17.0.1/tcp/30334/ws
2024-08-24 23:51:12.835  INFO tokio-runtime-worker libp2p_mdns::behaviour: discovered: 12D3KooWSHAYsRX8zqb9Jkm29Z4hdhqt8Z8fazQS3iD3wKaJQBWS /ip4/192.168.31.180/tcp/30334/ws
2024-08-24 23:51:12.835  INFO tokio-runtime-worker libp2p_mdns::behaviour: discovered: 12D3KooWSHAYsRX8zqb9Jkm29Z4hdhqt8Z8fazQS3iD3wKaJQBWS /ip4/172.18.0.1/tcp/30334/ws
2024-08-24 23:51:12.836  INFO tokio-runtime-worker substrate: 💤 Idle (0 peers), best: #67321 (0xd2ed…9e26), finalized #67315 (0x87b1…9f3d), ⬇ 0 ⬆ 0
2024-08-24 23:51:17.836  INFO tokio-runtime-worker substrate: 💤 Idle (1 peers), best: #67321 (0xd2ed…9e26), finalized #67315 (0x87b1…9f3d), ⬇ 24.0kiB/s ⬆ 7.6MiB/s
2024-08-24 23:51:22.020  INFO tokio-runtime-worker substrate: 🏆 Imported #67322 (0xd2ed…9e26 → 0x5795…29ab)
2024-08-24 23:51:22.020  INFO tokio-runtime-worker subcoin_service::finalizer: ✅ Successfully finalized block #67316,0x71bf…452d
2024-08-24 23:51:22.022  INFO tokio-runtime-worker substrate: 🏆 Imported #67323 (0x5795…29ab → 0xa47e…5c4e)
2024-08-24 23:51:22.022  INFO tokio-runtime-worker subcoin_service::finalizer: ✅ Successfully finalized block #67317,0x7309…96e4
2024-08-24 23:51:22.089  INFO tokio-runtime-worker substrate: 🏆 Imported #67324 (0xa47e…5c4e → 0xea73…0486)
2024-08-24 23:51:22.089  INFO tokio-runtime-worker subcoin_service::finalizer: ✅ Successfully finalized block #67318,0x875e…0bf2
2024-08-24 23:51:22.164  INFO tokio-runtime-worker substrate: 🏆 Imported #67325 (0xea73…0486 → 0x1ba2…0a86)
2024-08-24 23:51:22.164  INFO tokio-runtime-worker subcoin_service::finalizer: ✅ Successfully finalized block #67319,0xe412…f0b1
2024-08-24 23:51:22.237  INFO tokio-runtime-worker substrate: 🏆 Imported #67326 (0x1ba2…0a86 → 0x6c25…ce85)
```

This setup confirms that the Substrate fast sync mechanism is operational, even if syncing to the latest Bitcoin state is currently limited by the size of the state.

## Quick Testing

A public bootnode is deployed on the cloud so that you can quickly verify that the fast sync works without going through the entire local setup:

```bash
./target/release/subcoin run -d fast-sync-node-data --sync fast-unsafe --state-pruning=256 --blocks-pruning=256 --log subcoin_network=debug,sync=debug --port 30334 --bootnodes /ip4/45.55.202.218/tcp/30000/ws/p2p/12D3KooWA3TFwBczFcwgZABAJJJBAJgunDwJoe55jgfkDuz8KQsb
```
