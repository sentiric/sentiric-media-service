# --- STAGE 1: Builder ---
FROM rust:1.88-bullseye AS builder

# Gerekli TÜM derleme araçlarını en başta kuruyoruz.
RUN apt-get update && \
    apt-get install -y \
    build-essential \
    libssl-dev \
    pkg-config \
    protobuf-compiler \
    git \
    curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Önce sadece bağımlılık tanımlarını kopyala (Hafif cache optimizasyonu)
COPY Cargo.toml Cargo.lock ./

# Bağımlılıkları indir
RUN cargo fetch

# Şimdi tüm kaynak kodunu kopyala
COPY . .

# Derlemeyi yap
RUN cargo build --release

# --- STAGE 2: Final (Minimal) Image ---
FROM debian:bookworm-slim

# Gerekli runtime bağımlılıkları
RUN apt-get update && apt-get install -y netcat-openbsd ca-certificates && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Derlenmiş binary'yi kopyala
COPY --from=builder /app/target/release/sentiric-media-service .

# Güvenlik için kullanıcı oluştur ve kullan
RUN useradd -m -u 1001 appuser
USER appuser

# Servisi başlat
ENTRYPOINT ["./sentiric-media-service"]