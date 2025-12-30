#!/bin/sh
set -e

# --- DEĞİŞİKLİK BURADA BAŞLIYOR ---
# BUCKET_PROVIDER değişkenini kontrol et. Eğer ayarlanmamışsa, varsayılan olarak "minio" kabul et.
BUCKET_PROVIDER="${BUCKET_PROVIDER:-minio}"

if [ "$BUCKET_PROVIDER" = "minio" ]; then
  echo "--- 🕒 Waiting for MinIO to be healthy (Provider: minio)... ---"
  # MinIO'ya doğrudan IP adresi ile erişim
  while ! curl -f "http://${MINIO_HOST}:${MINIO_API_PORT}/minio/health/live"; do
      echo "MinIO is not ready at ${MINIO_HOST}. Retrying in 2 seconds..."
      sleep 2
  done
  echo "--- ✅ MinIO is ready! ---"
else
  echo "--- ℹ️ Bucket Provider is '$BUCKET_PROVIDER'. Skipping MinIO health check. ---"
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
if [ "$BUCKET_PROVIDER" = "minio" ]; then
  echo "\n--- 🛠️ Configuring MinIO... ---"
  # mc komutu için de doğrudan IP kullanmak en garantisi.
  mc alias set myminio "http://${MINIO_HOST}:${MINIO_API_PORT}" "${MINIO_ROOT_USER}" "${MINIO_ROOT_PASSWORD}" --quiet
  echo "Creating bucket: ${BUCKET_NAME}"
  mc mb "myminio/${BUCKET_NAME}" --ignore-existing
  echo "Setting anonymous policy for bucket: ${BUCKET_NAME}"
  mc anonymous set public "myminio/${BUCKET_NAME}"
  echo "--- ✅ MinIO configuration complete. ---"
else
  echo "\n--- ℹ️ Bucket Provider is '$BUCKET_PROVIDER'. Skipping MinIO bucket creation. ---"
  echo "---    (Assuming bucket '${BUCKET_NAME}' already exists on the provider) ---"
fi
# --- DEĞİŞİKLİK SONU ---

echo "\n\n--- 🧪 Starting All Tests ---"

# NOT: .env.test .env.example .env gibi dev ortamları yada docker ortamında karmaşıklık var!!!
# test ortamında yada genelinde environment tanımları düzensiz

# docker-compose.test sırasında hata
# thread 'main' panicked at examples/agent_client.rs:21:13:
# '.env.example' dosyası yüklenemedi: path not found
# note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

# cargo run --example agent_client ile local çalışıyor?

echo "\n\n--- 🧪 TEST : Agent Client Doğrulama"
./agent_client

echo "\n\n--- 🧪 TEST : Dialplan Client Doğrulama"
./dialplan_client

echo "\n\n--- 🧪 TEST : Uçtan Uca Temel Diyalog Doğrulama"
./end_to_end_call_validator

echo "\n\n--- 🧪 TEST : Live Audio Client Doğrulama"
./live_audio_client

echo "\n\n--- 🧪 TEST : Gerçekçi Çağrı Akışı (Anons Kuyruğu ve Cızırtı) Doğrulama"
./realistic_call_flow

echo "\n\n--- 🧪 TEST : Record Client Doğrulama"
./recording_client

echo "\n\n--- 🧪 TEST : Sip Signaling Client Doğrulama"
./sip_signaling_client

echo "\n\n--- 🧪 TEST : TTS Stream Doğrulama"
./tts_stream_client

echo "\n\n--- 🧪 TEST : User Client Doğrulama"
./user_client

echo "\n\n--- 🧪 TEST : CAll Simulator Doğrulama"
./call_simulator

echo "\n--- ✅✅✅ ALL TESTS PASSED SUCCESSFULLY --- ✅✅✅"