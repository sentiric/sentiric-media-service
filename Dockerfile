# --- STAGE 1: Builder ---
# bullseye imajını kullanıyoruz çünkü daha stabil ve yaygın build araçları içeriyor.
FROM rust:1.88-bullseye AS builder

# Gerekli TÜM derleme araçlarını en başta kuruyoruz.
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    libssl-dev \
    pkg-config \
    protobuf-compiler \
    git \
    curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Önce sadece bağımlılık tanımlarını kopyala.
COPY Cargo.toml Cargo.lock ./

# --- DÜZELTME BAŞLANGICI ---
# Sadece bağımlılıkları indirmek için, Cargo'ya derleyecek bir şey olmadığını ama yine de
# bağımlılıkları hazırlamasını söylememiz gerekiyor.
# Bunun için geçici, boş bir main.rs dosyası oluşturuyoruz.
RUN mkdir -p src && echo "fn main() {}" > src/main.rs

# Bu komut, bağımlılıkları derleyerek Docker katmanında cache'lenmesini sağlar.
# Bu en uzun süren adımdır ve sadece Cargo.lock değiştiğinde çalışır.
RUN cargo build --release
# Derleme sonrası geçici dosyayı ve kendi derlenmiş kodumuzu temizleyelim ki bir sonraki adımda sadece bağımlılıklar kalsın.
RUN rm -rf src target/release/deps/sentiric_media_service*
# --- DÜZELTME SONU ---

# Şimdi tüm gerçek kaynak kodunu kopyala
COPY . .

# Bu son derleme adımı, önbelleğe alınmış bağımlılıkları kullanarak sadece
# kendi uygulama kodumuzu derleyeceği için ÇOK HIZLI olacaktır.
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