# --- STAGE 1: Planner - Sadece bağımlılıkları planlamak için ---
# Bu aşama, bağımlılıkları derlemeden önce sadece "planlar".
# Bu, Docker'ın daha iyi cache yapmasını sağlar.
FROM rust:1.88-slim-bookworm AS planner
WORKDIR /app
RUN cargo init --bin

# Sadece bağımlılık tanımlarını kopyala
COPY Cargo.toml Cargo.lock ./

# Sadece bağımlılıkları çöz, derleme yapma. Bu çok hızlı bir adımdır.
RUN cargo build --release --locked --out-dir=./out

# --- STAGE 2: Builder - Bağımlılıkları ve kodu derlemek için ---
FROM rust:1.88-slim-bookworm AS builder

# Gerekli sistem bağımlılıklarını kur
RUN apt-get update && \
    apt-get install -y \
    build-essential \
    pkg-config \
    libssl-dev \
    # Not: Bu aşamada artık protobuf-compiler, git, curl'e ihtiyacımız olmayabilir
    # çünkü kontratlar ve diğer her şey zaten context'te var. Temiz tutalım.
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Önce sadece bağımlılıkları kopyala
COPY Cargo.toml Cargo.lock ./

# Planner aşamasından derlenmiş bağımlılıkları kopyala (bu cache'lenir)
COPY --from=planner /app/target/release/deps/ ./target/release/deps/

# Şimdi tüm kaynak kodunu kopyala
# Sadece .rs dosyaları değiştiğinde bu ve sonraki katmanlar yeniden çalışır.
COPY src ./src
COPY examples ./examples
COPY build.rs ./build.rs
# sentiric-contracts submodule'ünü de kopyalamak için (eğer varsa)
# COPY sentiric-contracts ./sentiric-contracts

# Son olarak, sadece kendi kodumuzu derle (bağımlılıklar zaten derlendi)
RUN cargo build --release --locked

# --- STAGE 3: Final - Çalıştırılabilir minimal imaj ---
FROM debian:bookworm-slim AS final

# Gerekli runtime bağımlılıklarını kur (healthcheck için netcat)
RUN apt-get update && apt-get install -y netcat-openbsd ca-certificates && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Sadece ve sadece derlenmiş binary dosyasını kopyala
COPY --from=builder /app/target/release/sentiric-media-service .

# Güvenlik için root olmayan bir kullanıcıyla çalıştır
RUN useradd -m -u 1001 appuser
USER appuser

# Servisi başlat
ENTRYPOINT ["./sentiric-media-service"]