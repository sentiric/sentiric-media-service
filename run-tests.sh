#!/bin/sh
set -e

# --- YENİ KURULUM BÖLÜMÜ ---

# MinIO'nun sağlık kontrolü endpoint'inin hazır olmasını bekle
echo "--- 🕒 Waiting for MinIO to be healthy... ---"
while ! curl -f "http://${MINIO_HOST}:${MINIO_API_PORT}/minio/health/live"; do
    echo "MinIO is not ready yet. Retrying in 2 seconds..."
    sleep 2
done
echo "--- ✅ MinIO is ready! ---"

# MinIO'yu yapılandır
echo "\n--- 🛠️ Configuring MinIO... ---"
mc alias set myminio "http://${MINIO_HOST}:${MINIO_API_PORT}" "${MINIO_ROOT_USER}" "${MINIO_ROOT_PASSWORD}"
echo "Creating bucket: ${S3_BUCKET_NAME}"
mc mb "myminio/${S3_BUCKET_NAME}" --ignore-existing
echo "Setting public policy for bucket: ${S3_BUCKET_NAME}"
mc policy set public "myminio/${S3_BUCKET_NAME}"
echo "--- ✅ MinIO configuration complete. ---"

# --- KURULUM BÖLÜMÜ SONU ---

echo "\n\n--- 🧪 Starting Tests ---"

# # Temel Bağlantı ve API Testleri
# echo "\n\n--- 🧪 Tüm Ortam Değişkenlerini Doğrula ---"
# ./test

# echo "\n\n--- 🧪 Agent Service gibi davran (Port Al/Bırak)"
# ./agent_client

# echo "\n\n--- 🧪 SIP Signaling gibi davran (Kapasite Kontrolü)"
# ./sip_signaling_client

# echo "\n\n--- 🧪 Dialplan gibi davran (Anons Çal)"
# ./dialplan_client

# echo "\n\n--- 🧪 User Service gibi davran (Sadece Bağlantı Testi)"
# ./user_client

# # Fonksiyonel Senaryo Testleri
# echo "\n\n--- 🧪 Canlı Ses Akışını Test Et (STT Simülasyonu)"
# ./live_audio_client

# echo "\n\n--- 🧪 S3'e Kalıcı Çağrı Kaydını Test Et"
# ./recording_client

# echo "\n\n--- 🧪 Performans ve Stres Testi Uygula"
# ./call_simulator

# En Kapsamlı Manuel Test [ FOCUS END TO END TEST]
echo "\n\n--- 🧪 Uçtan Uca Diyalog ve Ses Birleştirmeyi Doğrula"
./end_to_end_call_validator

echo "\n--- ✅ ALL TESTS PASSED SUCCESSFULLY ---"