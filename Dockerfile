# --- STAGE 1: Planner - Sadece bağımlılıkları planlamak için ---
FROM rust:1.88-slim-bookworm AS planner
WORKDIR /app
# Bağımlılıkları derlemek için dummy bir proje oluştur.
RUN cargo init --bin

# Sadece bağımlılık tanımlarını kopyala.
COPY Cargo.toml Cargo.lock ./

# DÜZELTME: Planner aşaması, protoc dahil tüm build bağımlılıklarına ihtiyaç duyar.
RUN apt-get update && apt-get install -y \
    build-essential \
    libssl-dev \
    pkg-config \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

# Bağımlılıkları önceden derle.
RUN cargo build --release --locked
# Kendi kodumuzu temizle.
RUN rm -f target/release/deps/planner*

# --- STAGE 2: Builder - Bağımlılıkları ve kodu derlemek için ---
FROM rust:1.88-slim-bookworm AS builder

# Gerekli sistem bağımlılıklarını kur (Burada da protoc gerekli olabilir, garanti olsun diye ekliyoruz)
RUN apt-get update && \
    apt-get install -y \
    build-essential \
    libssl-dev \
    pkg-config \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Önce sadece bağımlılık tanımlarını tekrar kopyala
COPY Cargo.toml Cargo.lock ./

# Planner aşamasından ÖNCEDEN DERLENMİŞ bağımlılıkları kopyala.
COPY --from=planner /app/target/release/deps/ ./target/release/deps/

# Şimdi SADECE var olan kaynak kodunu kopyala
COPY src ./src
COPY examples ./examples

# Son olarak, sadece kendi kodumuzu derle
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