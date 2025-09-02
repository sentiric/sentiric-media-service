# --- STAGE 1: Planner - Sadece bağımlılıkları planlamak için ---
FROM rust:1.88-slim-bookworm AS planner
WORKDIR /app
RUN cargo init --bin

# Gerekli tüm derleme bağımlılıklarını bu aşamaya ekliyoruz
RUN apt-get update && apt-get install -y \
    build-essential \
    libssl-dev \
    pkg-config \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

# Sadece bağımlılık tanımlarını kopyala.
COPY Cargo.toml Cargo.lock ./

# Bağımlılıkları önceden derle.
RUN cargo build --release --locked
# Kendi kodumuzu temizle.
RUN rm -f target/release/deps/planner*

# --- STAGE 2: Builder - Bağımlılıkları ve kodu derlemek için ---
FROM rust:1.88-slim-bookworm AS builder

# Gerekli sistem bağımlılıklarını buraya da ekliyoruz (en güvenli yöntem)
RUN apt-get update && \
    apt-get install -y \
    build-essential \
    libssl-dev \
    pkg-config \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY --from=planner /app/target/release/deps/ ./target/release/deps/
COPY src ./src
COPY examples ./examples

RUN cargo build --release --locked

# --- STAGE 3: Final - Çalıştırılabilir minimal imaj ---
FROM debian:bookworm-slim AS final

RUN apt-get update && apt-get install -y netcat-openbsd ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/target/release/sentiric-media-service .
RUN useradd -m -u 1001 appuser
USER appuser
ENTRYPOINT ["./sentiric-media-service"]