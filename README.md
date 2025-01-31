<div align="center">

<p align="center"><img width="400" src="./docs/src/images/subcoin-high-resolution-logo.png" alt="Subcoin logo"></p>

[![Continuous integration](https://github.com/subcoin-project/subcoin/actions/workflows/ci.yml/badge.svg)](https://github.com/subcoin-project/subcoin/actions/workflows/ci.yml)
[![Docs](https://github.com/subcoin-project/subcoin/actions/workflows/docs.yml/badge.svg)](https://github.com/subcoin-project/subcoin/actions/workflows/docs.yml)
[![Subcoin Book](https://img.shields.io/badge/User%20Guide-blue?logo=mdBook&logoColor=%23292b2e&link=https%3A%2F%2Fsubcoin-project.github.io%2Fsubcoin%2Fbook)](https://subcoin-project.github.io/subcoin/book)
[![Telegram](https://img.shields.io/badge/Telegram-blue?color=gray&logo=telegram&logoColor=%#64b5ef)](https://t.me/subcoin_project)

</div>

> [!WARNING]
>
> Subcoin is currently in its early development stages and is not yet ready for production use.
> See [the disclaimer below](#disclaimer).

## Overview

Subcoin is a full node implementation of Bitcoin in Rust, built using the [polkadot-sdk (formerly Substrate)](https://github.com/paritytech/polkadot-sdk) framework.
By leveraging Substrate's modular and flexible architecture, Subcoin aims to offer a robust
and performant implementation of Bitcoin protocol. It is the first Bitcoin client that
introduces the decentralized fast sync into the Bitcoin ecosystem, a syncing strategy common
in newer blockchains (Ethereum, Polkadot, Near, etc) with a global chain state.

## Features

- 🔄 **Fast Sync**. Employs Substrate's advanced state sync strategy, enabling Bitcoin fast sync
  by downloading all headers and the state at a specific block, in decentralized manner. This allows
  new users to quickly sync to the latest state of the Bitcoin chain by running a Subcoin node.

- 🔗 **Substrate Integration**. Utilizes Substrate framework to provide production-level blockchain infrastructures.

## Development Status

Subcoin currently includes a tentative implementation for syncing as a Bitcoin full node, along with an
early version of the fast sync feature. However, due to limitations, syncing to the tip of the Bitcoin
network using fast sync is not feasible when the chain state becomes large. We are actively exploring
solutions to address this limitation, which will be supported in future updates.

It's important to note that Subcoin is still under active development and is not yet stable. It also
does not yet support participation in Bitcoin consensus as a miner node. Additional features and
improvements are planned for future releases.

## Run Tests

```bash
cargo test --workspace --all
```

## Disclaimer

**Do not use Subcoin in production.** It is a heavy work in progress, not feature-complete and the code
has not been audited as well. Use at your own risk.

## Contributing

Contributions to Subcoin are welcome! If you have ideas for improvements, bug fixes, or new features,
feel free to open an issue or submit a pull request. Make sure to follow the contribution guidelines
and code of conduct when contributing to the project.

## Acknowledgements

Subcoin builds on the work of several open-source Bitcoin projects. We are grateful to the developers and communities behind:

- **[Bitcoin Core](https://github.com/bitcoin/bitcoin):** The original Bitcoin implementation.
- **[btcd](https://github.com/btcsuite/btcd):** A Go-based Bitcoin full node.
- **[Parity Bitcoin](https://github.com/paritytech/parity-bitcoin):** A Rust-based Bitcoin implementation.
- **[rust-bitcoin](https://github.com/rust-bitcoin/rust-bitcoin):** A library for Bitcoin protocol in Rust.

We also thank the broader Bitcoin communities for their contributions to decentralized technology. If we missed anyone, please let us know!

## Donations

Subcoin is an open-source public good project dedicated to advancing decentralized technologies around Bitcoin.
We are grateful for receiving the [grant](https://github.com/w3f/Grants-Program/pull/2304) of the Web3 Foundation
for our initial development.

If you believe in our mission and would like to contribute to the continued growth and improvement of Subcoin,
we welcome your support:

- BTC: `bc1p9nvx58550rlhk29a6urzxg06hmgv9sh5afwdt66md09xwpdytlmqun0j4v`
- DOT: `12uXLCZZkprwRBhfmhTXdfQE8faQwgNpS76wwmnEbgDwWB9e`

## License

Subcoin is licensed under the [MIT License](LICENSE). See the LICENSE file for information.
