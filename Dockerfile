# --- AŞAMA 1: Derleme (Builder) ---
FROM rust:1.79-alpine AS builder

# build-essential, git gibi temel araçları kur
RUN apk add --no-cache build-base git protobuf-dev

WORKDIR /app

# YENİ ADIM: Asset'leri klonla
RUN git clone https://github.com/sentiric/sentiric-assets.git

COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release

COPY src ./src
RUN cargo build --release

# --- AŞAMA 2: Çalıştırma (Runtime) ---
FROM alpine:latest
RUN apk add --no-cache libc6-compat

WORKDIR /app

# Derlenmiş uygulamayı kopyala
COPY --from=builder /app/target/release/sentiric-media-service .

# YENİ ADIM: Asset'leri builder'dan kopyala
COPY --from=builder /app/sentiric-assets/audio /app/assets

ENTRYPOINT ["./sentiric-media-service"]