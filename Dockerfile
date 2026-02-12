# --- STAGE 1: Builder ---
# [KRİTİK GÜNCELLEME]: Rust versiyonu 1.88'den güncel bir kararlı sürüme yükseltildi.
# Bu, yerel ve CI ortamları arasındaki tutarlılığı sağlar.
FROM rust:1.93-slim-bookworm AS builder

# Gerekli derleme araçlarını kur
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

# Build argümanlarını bu aşamada tanımla
ARG GIT_COMMIT
ARG BUILD_DATE
ARG SERVICE_VERSION

WORKDIR /app

COPY Cargo.toml Cargo.lock ./

RUN mkdir src && \
    echo "fn main() {}" > src/main.rs && \
    cargo build --release --quiet && \
    rm -rf src target/release/deps/sentiric_media_service*

COPY . .

ENV GIT_COMMIT=${GIT_COMMIT}
ENV BUILD_DATE=${BUILD_DATE}
ENV SERVICE_VERSION=${SERVICE_VERSION}

RUN cargo build --release

# --- STAGE 2: Final (Minimal) Image ---
FROM debian:bookworm-slim

# --- Çalışma zamanı sistem bağımlılıkları ---
RUN apt-get update && apt-get install -y --no-install-recommends \
    netcat-openbsd \
    curl \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/* 

# Build argümanlarını final stage'de TEKRAR TANIMLA
ARG GIT_COMMIT
ARG BUILD_DATE
ARG SERVICE_VERSION

# Argümanları environment değişkenlerine ata
ENV GIT_COMMIT=${GIT_COMMIT}
ENV BUILD_DATE=${BUILD_DATE}
ENV SERVICE_VERSION=${SERVICE_VERSION}

WORKDIR /app

# [DÜZELTME]: Komutlar yeniden sıralandı.
# 1. Önce kullanıcıyı oluştur.
RUN addgroup --system --gid 1001 appgroup && \
    adduser --system --no-create-home --uid 1001 --ingroup appgroup appuser

# 2. Dosyaları kopyala VE kopyalarken doğru sahibi ata (--chown).
COPY --from=builder --chown=appuser:appgroup /app/target/release/sentiric-media-service .

# 3. Artık 'RUN chown' komutuna gerek yok.

# 4. Kullanıcıyı değiştir.
USER appuser

ENTRYPOINT ["./sentiric-media-service"]
