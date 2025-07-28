# --- AŞAMA 1: Derleme (Builder) ---
FROM rust:1.88-slim-bookworm AS builder

# Gerekli derleme araçlarını VE git'i kuruyoruz.
RUN apt-get update && apt-get install -y protobuf-compiler clang libclang-dev pkg-config git

WORKDIR /app

# YENİ ADIM: Asset'leri derleme ortamına klonla
RUN git clone https://github.com/sentiric/sentiric-assets.git

# Bağımlılıkları önbelleğe almak için önce sadece Cargo dosyalarını kopyala
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release

# Kaynak kodunu kopyala ve asıl derlemeyi yap
COPY src ./src
RUN cargo build --release

# --- AŞAMA 2: Çalıştırma (Runtime) ---
FROM gcr.io/distroless/cc-debian12

WORKDIR /app

# Derlenmiş uygulamayı builder aşamasından kopyala
COPY --from=builder /app/target/release/sentiric-media-service .

# YENİ ADIM: Klonlanmış asset'leri builder aşamasından son imajın içine kopyala
# Bu, konteyner içinde /app/assets/audio/... yolunu oluşturur.
COPY --from=builder /app/sentiric-assets/audio /app/assets/audio

# .env'den gelen portları EXPOSE etmek iyi bir pratiktir.
EXPOSE 50052/tcp
EXPOSE 10000-20000/udp 

ENTRYPOINT ["./sentiric-media-service"]