# --- AŞAMA 1: DERLEME (BUILDER) ---
# Kodumuzu standart ve temiz bir ortamda derliyoruz.
FROM rust:1.88-slim-bookworm AS builder

# Gerekli tüm derleme araçlarını kuruyoruz.
RUN apt-get update && apt-get install -y protobuf-compiler git && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Bağımlılık katmanını oluşturuyoruz. Bu katman sadece bağımlılıklar değiştiğinde yeniden çalışır.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release

# Asıl kaynak kodunu kopyalıyoruz.
COPY ./src ./src

# Asset'leri de bu aşamada klonluyoruz.
RUN git clone https://github.com/sentiric/sentiric-assets.git

# Nihai derlemeyi yapıyoruz. Bu adım, kaynak kodu değiştiğinde çalışır ve hızlıdır.
RUN cargo build --release


# --- AŞAMA 2: ÇALIŞTIRMA (RUNTIME) ---
# Çalıştırma ortamı olarak küçük ve güvenilir bir Debian imajı kullanıyoruz.
FROM debian:bookworm-slim

# Çalışma zamanında ihtiyaç duyulabilecek temel kütüphaneleri kuruyoruz.
# RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Sadece derlenmiş, optimize edilmiş binary'yi ve asset'leri builder'dan kopyalıyoruz.
COPY --from=builder /app/target/release/sentiric-media-service .
COPY --from=builder /app/sentiric-assets/audio /app/assets/audio

# .env dosyasından gelen portları belirtmek, iyi bir dokümantasyon ve ağ kuralı yönetimi pratiğidir.
EXPOSE 50052/tcp
EXPOSE 10000-10010/udp

# ÖNEMLİ: Hata ayıklama araçlarını (ldd ve strace) kuruyoruz.
# 'procps' ldd için gereklidir.
# RUN apt-get update && apt-get install -y ca-certificates procps strace && rm -rf /var/lib/apt/lists/*
# CMD ["tail", "-f", "/dev/null"]
# ldd ./sentiric-media-service
# strace ./sentiric-media-service

# Konteyner başladığında bu binary'yi çalıştır.
ENTRYPOINT ["./sentiric-media-service"]
