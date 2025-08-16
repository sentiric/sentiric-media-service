# --- STAGE 1: Builder ---
FROM rust:1.88-slim-bookworm AS builder

# Gerekli derleme araçlarını ve YENİ olarak buf CLI'ı kuruyoruz.
RUN apt-get update && \
    apt-get install -y protobuf-compiler git curl && \
    curl -sSL https://github.com/bufbuild/buf/releases/latest/download/buf-Linux-x86_64 -o /usr/local/bin/buf && \
    chmod +x /usr/local/bin/buf && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Bağımlılıkları önbelleğe al
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && echo "fn main() {}" > src/main.rs
RUN cargo build --release

# Kaynak kodunu kopyala ve derle
COPY . .
RUN cargo build --release

# Asset'leri klonla
RUN git clone --depth 1 https://github.com/sentiric/sentiric-assets.git assets

# --- STAGE 2: Final (Minimal) Image ---
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y netcat-openbsd ca-certificates && rm -rf /var/lib/apt/lists/*

ARG SERVICE_NAME
WORKDIR /app

COPY --from=builder /app/assets/audio ./assets/audio
COPY --from=builder /app/target/release/${SERVICE_NAME} .
USER 10001
ENTRYPOINT ["./sentiric-media-service"]