#!/bin/sh
set -e

echo "--- 🕒 Waiting for MinIO to be healthy... ---"
while ! curl -f "http://${MINIO_HOST}:${MINIO_API_PORT}/minio/health/live"; do
    echo "MinIO is not ready yet. Retrying in 2 seconds..."
    sleep 2
done
echo "--- ✅ MinIO is ready! ---"

# YENİ: Media Service'in gRPC portunun da hazır olmasını bekle
echo "\n--- 🕒 Waiting for Media Service to be healthy... ---"
while ! nc -z "${MEDIA_SERVICE_HOST}" "${MEDIA_SERVICE_GRPC_PORT}"; do
    echo "Media Service gRPC port is not ready yet. Retrying in 2 seconds..."
    sleep 2
done
echo "--- ✅ Media Service is ready! ---"

echo "\n--- 🛠️ Configuring MinIO... ---"
# mc'nin anonim kullanım uyarısını bastırmak için --quiet eklendi
mc alias set myminio "http://${MINIO_HOST}:${MINIO_API_PORT}" "${MINIO_ROOT_USER}" "${MINIO_ROOT_PASSWORD}" --quiet
echo "Creating bucket: ${S3_BUCKET_NAME}"
mc mb "myminio/${S3_BUCKET_NAME}" --ignore-existing
echo "Setting anonymous policy for bucket: ${S3_BUCKET_NAME}"
# DÜZELTME: Doğru mc komutu `mc anonymous set public`
mc anonymous set public "myminio/${S3_BUCKET_NAME}"
echo "--- ✅ MinIO configuration complete. ---"


echo "\n\n--- 🧪 Starting All Tests ---"

echo "\n\n--- 🧪 TEST 1: Uçtan Uca Temel Diyalog Doğrulama"
./end_to_end_call_validator

echo "\n\n--- 🧪 TEST 2: Gerçekçi Çağrı Akışı (Anons Kuyruğu ve Cızırtı) Doğrulama"
./realistic_call_flow

echo "\n--- ✅✅✅ ALL TESTS PASSED SUCCESSFULLY --- ✅✅✅"