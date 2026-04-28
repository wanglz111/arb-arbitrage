FROM rust:1.95-bookworm AS builder

ARG SCANNER_BUILD_GIT_SHA=unknown
ARG SCANNER_BUILD_GIT_REF=unknown
ARG SCANNER_BUILD_CREATED=unknown

WORKDIR /app

COPY scanner/Cargo.toml scanner/Cargo.lock ./scanner/
COPY scanner/src/main.rs ./scanner/src/main.rs
RUN cargo fetch --manifest-path scanner/Cargo.toml

COPY scanner ./scanner
RUN cargo build --manifest-path scanner/Cargo.toml --release

FROM debian:bookworm-slim AS runtime

ARG SCANNER_BUILD_GIT_SHA=unknown
ARG SCANNER_BUILD_GIT_REF=unknown
ARG SCANNER_BUILD_CREATED=unknown

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/* && \
    useradd --create-home --uid 10001 appuser && \
    mkdir -p /app/data && \
    chown -R appuser:appuser /app

WORKDIR /app

COPY --from=builder /app/scanner/target/release/scanner /usr/local/bin/scanner

USER appuser

ENV RUST_LOG=scanner=info \
    SCANNER_BUILD_GIT_SHA=${SCANNER_BUILD_GIT_SHA} \
    SCANNER_BUILD_GIT_REF=${SCANNER_BUILD_GIT_REF} \
    SCANNER_BUILD_CREATED=${SCANNER_BUILD_CREATED}

ENTRYPOINT ["scanner"]
