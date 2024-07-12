# Installing Subcoin

## Compile from source

### Install prerequisites

Since subcoin is a Substrate-based chain, you must install the necessary development tools to compile the binary from source.
Please refer to https://docs.substrate.io/install/ for the full guide of installing the prerequisites.

### Compile subcoin node binary

Once the required packages and Rust installed, we can proceed to compile the binary.

```bash
cargo build --profile production --bin subcoin
```

The Subcoin node executable `subcoin` should be present at `target/production/subcoin`.

## Docker

```
```
