#!/bin/sh

echo "🧪 Binary Tipi:"
file ./sentiric-media-service

echo "🔍 Statik/Dinamik:"
ldd ./sentiric-media-service || echo "Statik olabilir"
readelf -l ./sentiric-media-service | grep interpreter || echo "Interpreter yok → statik"

echo "🔧 Ortam Değişkenleri:"
env | grep -i grpc

echo "🚀 Test Başlatılıyor:"
strace -o trace.log -f ./sentiric-media-service

tail -n 30 trace.log
