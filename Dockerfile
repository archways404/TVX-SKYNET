# syntax=docker/dockerfile:1

FROM rust:1.96-bookworm AS builder

WORKDIR /build

COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
RUN mkdir src \
    && printf 'fn main() {}\n' > src/main.rs \
    && cargo build --locked --release \
    && rm -rf src

COPY migrations ./migrations
COPY src ./src

RUN touch src/main.rs \
    && cargo build --locked --release

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --no-create-home --home-dir /app skynet

WORKDIR /app

COPY --from=builder /build/target/release/external-status-combinator /usr/local/bin/external-status-combinator
COPY config ./config

ENV EXTERNAL_STATUS_CONFIG=/app/config/sources.yaml \
    RUST_LOG=external_status_combinator=info,tower_http=info \
    STORAGE_BACKEND=postgres

USER 10001:10001

EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD ["curl", "--fail", "--silent", "--show-error", "http://127.0.0.1:8080/health/ready"]

ENTRYPOINT ["/usr/local/bin/external-status-combinator"]
