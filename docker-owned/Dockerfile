FROM rust:1.99.1-bookworm AS builder

WORKDIR /src
RUN apt-get update \
    && apt-get install --no-install-recommends -y libssl-dev libx11-dev pkg-config \
    && rm -rf /var/lib/apt/lists/*
COPY . .
RUN cargo build --locked --release --bin hbbs --bin hbbr

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install --no-install-recommends -y ca-certificates libssl3 libx11-6 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /src/target/release/hbbs /usr/local/bin/hbbs
COPY --from=builder /src/target/release/hbbr /usr/local/bin/hbbr

WORKDIR /root
ENV HOME=/root
EXPOSE 21115 21116 21116/udp 21117 21118 21119
