# Installing Subcoin

## Compile from source

### Install prerequisites

Since subcoin is a Substrate-based chain, you must install the necessary development tools to compile the binary from source.
Please refer to https://docs.substrate.io/install/ for the full guide on installing the prerequisites.

### Compile subcoin node binary

Once the required packages and Rust are installed, proceed to compile the binary:

```bash
cargo build --profile production --bin subcoin
```

The Subcoin node executable `subcoin` should be located at `target/production/subcoin`.

### Prebuilt executables

You can download the prebuilt executables from GitHub https://github.com/subcoin-project/subcoin/releases/tag/v0.1.0.

## Docker (Linux/amd64)

To use the Docker image for Subcoin, run the following command:

```bash
docker pull ghcr.io/subcoin-project/subcoin:v0.1.0
```
