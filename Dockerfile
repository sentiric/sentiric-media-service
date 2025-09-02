# --- STAGE 1: Builder ---
FROM rust:1.88-slim-bookworm AS builder

ARG CACHE_BREAK=default 

# Gerekli derleme araçlarını ve buf CLI'ı kuruyoruz.
# libssl-dev, openssl-sys crate'i için gereklidir.
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

WORKDIR /app

COPY . .

RUN cargo build --release

# --- STAGE 2: Final (Minimal) Image ---
FROM debian:bookworm-slim

# Sadece healthcheck için netcat ve SSL için ca-certificates gerekli.
RUN apt-get update && apt-get install -y netcat-openbsd ca-certificates && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/sentiric-media-service .

ENTRYPOINT ["./sentiric-media-service"]