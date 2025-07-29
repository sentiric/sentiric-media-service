# BASİT, GÜVENİLİR, TEK AŞAMALI DOCKERFILE

FROM rust:latest

# Gerekli tüm araçları en başta kur
RUN apt-get update && apt-get install -y protobuf-compiler git curl vim netcat-openbsd procps iproute2 lsof

WORKDIR /app

# Tüm projeyi kopyala
COPY . .

# Her şeyi derle
RUN cargo build --release


RUN git clone --depth 1 https://github.com/sentiric/sentiric-assets.git assets

EXPOSE 50052/tcp
EXPOSE 10000-10010/udp

# Doğru, derlenmiş binary'yi çalıştır
CMD ["/app/target/release/sentiric-media-service"]