# --- STAGE 1: Builder ---
FROM rust:1.93-slim-bookworm AS builder

RUN apt-get update && \
    apt-get install -y \
    protobuf-compiler \
    git \
    curl \
    libssl-dev \
    pkg-config \
    && \
    curl -sSL https://github.com/bufbuild/buf/releases/latest/download/buf-Linux-x86_64 -o /usr/local/bin/buf && \
    chmod +x /usr/local/bin/buf && \
    rm -rf /var/lib/apt/lists/*

ARG GIT_COMMIT
ARG BUILD_DATE
ARG SERVICE_VERSION

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && cargo build --release --quiet && rm -rf src target/release/deps/sentiric_media_service*
COPY . .
ENV GIT_COMMIT=${GIT_COMMIT} BUILD_DATE=${BUILD_DATE} SERVICE_VERSION=${SERVICE_VERSION}
RUN cargo build --release

# --- STAGE 2: Final ---
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    netcat-openbsd curl ca-certificates && rm -rf /var/lib/apt/lists/* 

ARG GIT_COMMIT
ARG BUILD_DATE
ARG SERVICE_VERSION

ENV GIT_COMMIT=${GIT_COMMIT} BUILD_DATE=${BUILD_DATE} SERVICE_VERSION=${SERVICE_VERSION}

WORKDIR /app

RUN addgroup --system --gid 1001 appgroup && \
    adduser --system --no-create-home --uid 1001 --ingroup appgroup appuser

# [KRİTİK DÜZELTME]: Kayıt dizinini yarat ve iznini ver ki rust kodu Permission Denied yemesin.
RUN mkdir -p /sentiric-media-recordings && chown appuser:appgroup /sentiric-media-recordings

COPY --from=builder --chown=appuser:appgroup /app/target/release/sentiric-media-service .

USER appuser
ENTRYPOINT ["./sentiric-media-service"]