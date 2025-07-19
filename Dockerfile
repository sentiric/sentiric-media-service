# Adım 1: Kodu derlemek için Go imajını kullan
FROM golang:1.22-alpine AS builder

WORKDIR /app

# Bağımlılıkları indir
COPY go.mod ./
RUN go mod download

# Kaynak kodunu kopyala
COPY . .

# Uygulamayı derle (statik olarak, C kütüphaneleri olmadan)
RUN CGO_ENABLED=0 GOOS=linux go build -a -installsuffix cgo -o sentiric-media-service .

# Adım 2: Sadece derlenmiş uygulamayı ve FFmpeg'i içeren minimal bir imaj oluştur
FROM alpine:latest

# FFmpeg'i kur
RUN apk --no-cache add ffmpeg

WORKDIR /root/

# Derlenmiş uygulamayı builder aşamasından kopyala
COPY --from=builder /app/sentiric-media-service .

# Portları aç
EXPOSE 3003
EXPOSE 10000-20000/udp

# Konteyner başladığında uygulamayı çalıştır
CMD ["./sentiric-media-service"]