# --- AŞAMA 1: Derleme (Builder) ---
FROM rust:1.79 as builder
RUN apt-get update && apt-get install -y protobuf-compiler clang libclang-dev

WORKDIR /app
COPY Cargo.toml Cargo.lock ./

# Sadece bağımlılıkları indirmek için sahte bir proje oluştur
RUN mkdir -p src && echo "fn main() {}" > src/main.rs
RUN cargo build --release
RUN rm -f target/release/deps/sentiric_media_service*

# Şimdi gerçek kaynak kodunu kopyala
COPY src ./src

# SİLİNEN SATIRLAR:
# build.rs ve core-interfaces artık burada kopyalanmıyor.
# COPY build.rs ./
# COPY ./core-interfaces ./core-interfaces

RUN cargo build --release

# --- AŞAMA 2: Çalıştırma (Runtime) ---
FROM gcr.io/distroless/cc-debian12
WORKDIR /app
COPY --from=builder /app/target/release/sentiric-media-service .
EXPOSE 50052/tcp
EXPOSE 10000-20000/udp
ENTRYPOINT ["/app/sentiric-media-service"]