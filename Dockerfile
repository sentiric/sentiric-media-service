# Dockerfile (Düzeltilmiş ve Güvenilir Multi-Stage)

# --- STAGE 1: Builder ---
# Bu aşama, kodu derlemek ve asset'leri indirmek için gerekli her şeyi yapar.
FROM rust:1.88-slim-bookworm AS builder

RUN apt-get update && apt-get install -y protobuf-compiler git && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Bağımlılıkları önbelleğe al
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && echo "fn main() {}" > src/main.rs
RUN cargo fetch

# Kaynak kodunu kopyala ve derle
COPY src ./src
RUN cargo build --release

# Asset'leri klonla
RUN git clone --depth 1 https://github.com/sentiric/sentiric-assets.git assets_repo


# --- STAGE 2: Final (Minimal) Image ---
# Bu aşama, SADECE builder'dan gerekli dosyaları alarak son imajı oluşturur.
FROM debian:bookworm-slim

# Çalışma dizinini oluştur
WORKDIR /app

# 1. Adım: Builder'dan derlenmiş binary'yi kopyala
COPY --from=builder /app/target/release/sentiric-media-service .

# 2. Adım: Builder'dan asset'leri kopyala
COPY --from=builder /app/assets_repo/audio ./assets/audio

EXPOSE 50052/tcp
EXPOSE 10000-10010/udp

# Uygulamayı çalıştır
CMD ["./sentiric-media-service"]