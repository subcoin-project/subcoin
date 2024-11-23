# This is a base image to build Subcoin node
FROM ubuntu:22.04 AS builder

ARG PROFILE=production
ARG SUBSTRATE_CLI_GIT_COMMIT_HASH

# Incremental compilation here isn't helpful
ENV CARGO_INCREMENTAL=0

WORKDIR /src

RUN apt-get update && \
    DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        ca-certificates \
        clang \
        cmake \
        curl \
        git \
        llvm \
        protobuf-compiler \
        make && \
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y

# Copy the source code
COPY . .

# Compile the binary and move it to /subcoin.
RUN /root/.cargo/bin/cargo build \
    --locked \
    --bin subcoin \
    --profile=$PROFILE \
    --target $(uname -p)-unknown-linux-gnu && \
    mv target/*/*/subcoin /subcoin && \
    rm -rf target

# This is the 2nd stage: a very small image where we copy the binary.
FROM ubuntu:22.04

LABEL org.opencontainers.image.source="https://github.com/subcoin-project/subcoin"
LABEL org.opencontainers.image.description="Multistage Docker image for Subcoin Node"

# Copy the node binary.
COPY --from=builder /subcoin /subcoin

RUN mkdir /node-data && chown nobody:nogroup /node-data

VOLUME ["/node-data"]

USER nobody:nogroup

EXPOSE 8333 30333 9933 9944 9615

ENTRYPOINT ["/subcoin"]
