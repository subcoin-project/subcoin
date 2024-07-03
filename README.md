# Subcoin: Bitcoin Full Node in Substrate

[![Continuous integration](https://github.com/subcoin-project/subcoin/actions/workflows/ci.yml/badge.svg)](https://github.com/subcoin-project/subcoin/actions/workflows/ci.yml)

> [!WARNING]
>
> Subcoin is currently in its early development stages and is not yet ready for production use.
> See [the disclaimer below](#disclaimer).

## Overview

Subcoin is a full node implementation of Bitcoin in Rust, built using the Substrate framework.
By leveraging Substrate's modular and flexible architecture, Subcoin aims to offer a robust
and performant implementation of Bitcoin protocol.

Currently, Subcoin only implements the feature of syncing as a full node. It is not yet capable
of participating in the Bitcoin consensus as a miner node. Additional features are in the
planning stages.

## Pros

- 🔄 **Fast Sync**. Employs Substrate's advanced state sync strategy, enabling Bitcoin fast sync
by downloading all headers and the state at a specific block.
- 🔗 **Substrate Integration**: Utilizes Substrate framework provide production-level blockchain infrastructures.

## Disclaimer

**Do not use Subcoin in production.** It is a heavy work in progress, not feature-complete and the code
has not been audited as well. Use at your own risk.

## Contributing

Contributions to Subcoin are welcome! If you have ideas for improvements, bug fixes, or new features,
feel free to open an issue or submit a pull request. Make sure to follow the contribution guidelines
and code of conduct when contributing to the project.

## License

Subcoin is licensed under the [MIT License](LICENSE). See the LICENSE file for information.
