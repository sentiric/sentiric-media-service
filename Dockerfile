# --- STAGE 1: Builder (Statik Derleme için MUSL Hedefi) ---
FROM rust:1.88-bullseye AS builder

# MUSL target'ını ve gerekli derleme araçlarını kuruyoruz (buf dahil).
RUN rustup target add x86_64-unknown-linux-musl
RUN apt-get update && \
    apt-get install -y musl-tools protobuf-compiler git curl && \
    curl -sSL https://github.com/bufbuild/buf/releases/latest/download/buf-Linux-x86_64 -o /usr/local/bin/buf && \
    chmod +x /usr/local/bin/buf && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Bağımlılıkları önbelleğe al
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && echo "fn main() {}" > src/main.rs
RUN cargo build --release --target x86_64-unknown-linux-musl

# Kaynak kodunu kopyala ve derle
COPY . .
RUN cargo build --release --target x86_64-unknown-linux-musl

# Asset'leri klonla
RUN git clone --depth 1 https://github.com/sentiric/sentiric-assets.git assets

# --- STAGE 2: Final (Sıfırdan İmaj) ---
FROM scratch

# Asset'leri ve sertifikaları alabilmek için ca-certificates'e ihtiyacımız var.
# Bu yüzden scratch yerine alpine'ın en küçüğünü kullanmak daha mantıklı.
FROM alpine:latest
RUN apk add --no-cache ca-certificates

ARG SERVICE_NAME
WORKDIR /app

# Asset'leri ve derlenmiş binary'yi kopyala
COPY --from=builder /app/assets/audio ./assets/audio
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/${SERVICE_NAME} .

USER 10001

ENTRYPOINT ["./sentiric-media-service"]