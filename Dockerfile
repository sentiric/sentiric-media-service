# --- Builder ---
FROM rust:latest AS builder

RUN apt-get update && apt-get install -yqq --no-install-recommends protobuf-compiler git musl-tools && rm -rf /var/lib/apt/lists/*
RUN rustup target add x86_64-unknown-linux-musl

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && \
    RUSTFLAGS='-C target-feature=+crt-static' cargo build --release --target=x86_64-unknown-linux-musl

COPY ./src ./src
RUN git clone --depth 1 https://github.com/sentiric/sentiric-assets.git

RUN RUSTFLAGS='-C target-feature=+crt-static' cargo build --release --target=x86_64-unknown-linux-musl


# --- Debug Runtime ---
FROM debian:bookworm-slim AS debug

WORKDIR /app

RUN apt-get update && apt-get install -y \
  musl-tools file strace curl vim netcat-openbsd procps iproute2 lsof libc-bin \
  && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/sentiric-media-service .
COPY --from=builder /app/sentiric-assets/audio /app/assets/audio

EXPOSE 50052/tcp
EXPOSE 10000-10010/udp

CMD ["tail", "-f", "/dev/null"]


# ---------------------------------------------------
# --- STAGE 2: Production (Statik & Minimal) ---
FROM gcr.io/distroless/static-debian12 AS production

WORKDIR /app

COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/sentiric-media-service .
COPY --from=builder /app/sentiric-assets/audio /app/assets/audio

EXPOSE 50052/tcp
EXPOSE 10000-10010/udp

ENTRYPOINT ["/app/sentiric-media-service"]

# ---------------------------------------------------
# --- STAGE 3: Development (Geliştirici Ortamı) ---
FROM debian:bookworm-slim AS development

WORKDIR /app

RUN apt-get update && apt-get install -y \
    curl \
    ca-certificates \
    vim \
    netcat-openbsd \
    procps \
    iproute2 \
    lsof \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/sentiric-media-service .
COPY --from=builder /app/sentiric-assets/audio /app/assets/audio

EXPOSE 50052/tcp
EXPOSE 10000-10010/udp

CMD ["./sentiric-media-service"]
