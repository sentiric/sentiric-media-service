# --- STAGE 1: Builder ---
FROM rust:1.88-slim-bookworm AS builder

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

COPY . .

# Build-time environment değişkenlerini ayarla ki Rust kodu bunları okuyabilsin (isteğe bağlı ama iyi pratik)
ENV GIT_COMMIT=${GIT_COMMIT}
ENV BUILD_DATE=${BUILD_DATE}
ENV SERVICE_VERSION=${SERVICE_VERSION}

# Derlemeyi yap
RUN cargo build --release

# --- STAGE 2: Final (Minimal) Image ---
FROM debian:bookworm-slim

# Sadece healthcheck için netcat ve SSL için ca-certificates gerekli.
RUN apt-get update && apt-get install -y netcat-openbsd ca-certificates && rm -rf /var/lib/apt/lists/*

# GÜVENLİK: Root olmayan bir kullanıcı oluştur
RUN addgroup --system --gid 1001 appgroup && \
    adduser --system --no-create-home --uid 1001 --ingroup appgroup appuser

# DÜZELTME: Build argümanlarını final stage'de TEKRAR TANIMLA
ARG GIT_COMMIT
ARG BUILD_DATE
ARG SERVICE_VERSION

# DÜZELTME: Argümanları environment değişkenlerine ata ki runtime'da erişilebilsin.
ENV GIT_COMMIT=${GIT_COMMIT}
ENV BUILD_DATE=${BUILD_DATE}
ENV SERVICE_VERSION=${SERVICE_VERSION}

WORKDIR /app

# Dosyaları kopyala ve sahipliği yeni kullanıcıya ver
COPY --from=builder /app/target/release/sentiric-media-service .
RUN chown appuser:appgroup ./sentiric-media-service

# GÜVENLİK: Kullanıcıyı değiştir
USER appuser

ENTRYPOINT ["./sentiric-media-service"]