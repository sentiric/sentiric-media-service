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

echo "\n\n--- 🧪 Starting Integration Tests ---"

echo "\n[1/3] Running Live Audio Stream Test (live_audio_client)..."
./live_audio_client

echo "\n[2/3] Running Persistent Recording Test (recording_client)..."
./recording_client

echo "\n[3/3] Running End-to-End Validation Test (end_to_end_call_validator)..."
./end_to_end_call_validator

echo "\n--- ✅ ALL TESTS PASSED SUCCESSFULLY ---"