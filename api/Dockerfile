FROM rust:1.91-bookworm AS builder

WORKDIR /build
COPY Cargo.toml Cargo.lock* ./
COPY src ./src
COPY web ./web
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo build --release --locked \
    && cp /build/target/release/rustdesk-owned-api /build/rustdesk-owned-api

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/rustdesk-owned-api /usr/local/bin/rustdesk-owned-api

USER 1000:1000
EXPOSE 21114
VOLUME ["/data"]

ENTRYPOINT ["/usr/local/bin/rustdesk-owned-api"]
