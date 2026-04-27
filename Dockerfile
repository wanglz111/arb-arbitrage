FROM rust:1.95-bookworm AS builder

WORKDIR /app

COPY scanner/Cargo.toml scanner/Cargo.lock ./scanner/
RUN cargo fetch --manifest-path scanner/Cargo.toml

COPY scanner ./scanner
RUN cargo build --manifest-path scanner/Cargo.toml --release

FROM debian:bookworm-slim AS runtime

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/* && \
    useradd --create-home --uid 10001 appuser

WORKDIR /app

COPY --from=builder /app/scanner/target/release/scanner /usr/local/bin/scanner

USER appuser

ENV RUST_LOG=scanner=info

ENTRYPOINT ["scanner"]
