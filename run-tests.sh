#!/bin/sh
set -e

# --- DEĞİŞİKLİK BURADA BAŞLIYOR ---
# S3_PROVIDER değişkenini kontrol et. Eğer ayarlanmamışsa, varsayılan olarak "minio" kabul et.
S3_PROVIDER="${S3_PROVIDER:-minio}"

if [ "$S3_PROVIDER" = "minio" ]; then
  echo "--- 🕒 Waiting for MinIO to be healthy (Provider: minio)... ---"
  # MinIO'ya doğrudan IP adresi ile erişim
  while ! curl -f "http://${MINIO_HOST}:${MINIO_API_PORT}/minio/health/live"; do
      echo "MinIO is not ready at ${MINIO_HOST}. Retrying in 2 seconds..."
      sleep 2
  done
  echo "--- ✅ MinIO is ready! ---"
else
  echo "--- ℹ️ S3 Provider is '$S3_PROVIDER'. Skipping MinIO health check. ---"
fi
# --- DEĞİŞİKLİK BURADA BİTİYOR ---

echo "\n--- 🕒 Waiting for Media Service to be healthy... ---"
# Media Service'e doğrudan IP adresi ile erişim
while ! nc -z "${MEDIA_SERVICE_HOST}" "${MEDIA_SERVICE_GRPC_PORT}"; do
    echo "Media Service (gRPC port) is not ready at ${MEDIA_SERVICE_HOST}. Retrying in 2 seconds..."
    sleep 2
done
echo "--- ✅ Media Service is ready! ---"

# --- DEĞİŞİKLİK BURADA BAŞLIYOR ---
if [ "$S3_PROVIDER" = "minio" ]; then
  echo "\n--- 🛠️ Configuring MinIO... ---"
  # mc komutu için de doğrudan IP kullanmak en garantisi.
  mc alias set myminio "http://${MINIO_HOST}:${MINIO_API_PORT}" "${MINIO_ROOT_USER}" "${MINIO_ROOT_PASSWORD}" --quiet
  echo "Creating bucket: ${S3_BUCKET_NAME}"
  mc mb "myminio/${S3_BUCKET_NAME}" --ignore-existing
  echo "Setting anonymous policy for bucket: ${S3_BUCKET_NAME}"
  mc anonymous set public "myminio/${S3_BUCKET_NAME}"
  echo "--- ✅ MinIO configuration complete. ---"
else
  echo "\n--- ℹ️ S3 Provider is '$S3_PROVIDER'. Skipping MinIO bucket creation. ---"
  echo "---    (Assuming bucket '${S3_BUCKET_NAME}' already exists on the provider) ---"
fi
# --- DEĞİŞİKLİK SONU ---

echo "\n\n--- 🧪 Starting All Tests ---"

echo "\n\n--- 🧪 TEST 1: Uçtan Uca Temel Diyalog Doğrulama"
./end_to_end_call_validator

echo "\n\n--- 🧪 TEST 2: Gerçekçi Çağrı Akışı (Anons Kuyruğu ve Cızırtı) Doğrulama"
./realistic_call_flow

echo "\n--- ✅✅✅ ALL TESTS PASSED SUCCESSFULLY --- ✅✅✅"