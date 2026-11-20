FROM rust:1.91-bookworm AS builder

WORKDIR /build
COPY Cargo.toml Cargo.lock* ./
COPY src ./src
RUN cargo build --release --locked

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/rustdesk-owned-api /usr/local/bin/rustdesk-owned-api

USER 1000:1000
EXPOSE 21114
VOLUME ["/data"]

ENTRYPOINT ["/usr/local/bin/rustdesk-owned-api"]
