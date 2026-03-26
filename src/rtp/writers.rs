// sentiric-media-service/src/rtp/writers.rs
use aws_sdk_s3::Client as S3Client;
use aws_sdk_s3::primitives::ByteStream;
use std::sync::Arc;
use tokio::time::{sleep, timeout, Duration}; // [ARCH-COMPLIANCE] timeout eklendi
use tracing::{info, error, warn, instrument};
use anyhow::Result;
use crate::metrics::S3_UPLOAD_FAILURES;
use metrics::counter;

#[instrument(skip(client, data), fields(s3.bucket = %bucket, s3.key = %key, file.size_bytes = data.len()))]
pub async fn upload_to_s3_with_retry(
    client: Arc<S3Client>,
    bucket: &str,
    key: &str,
    data: Vec<u8>,
) -> Result<()> {
    const MAX_RETRIES: u32 = 3;
    let mut attempt = 0;

    info!(event = "S3_UPLOAD_START", "☁️ Kayıt dosyası doğrudan bellekten S3'e yükleniyor...");

    loop {
        attempt += 1;
        let body = ByteStream::from(data.clone());
        
        //[ARCH-COMPLIANCE] Spec Kuralı: Dış I/O operasyonları explicit timeout içermelidir (15 Saniye)
        let s3_future = client.put_object().bucket(bucket).key(key).body(body).send();
        
        match timeout(Duration::from_secs(15), s3_future).await {
            Ok(Ok(_)) => {
                info!(event = "S3_UPLOAD_SUCCESS", attempt = attempt, "✅ Dosya S3'e başarıyla yüklendi.");
                return Ok(());
            }
            Ok(Err(e)) => {
                counter!(S3_UPLOAD_FAILURES).increment(1);
                
                if attempt >= MAX_RETRIES {
                    error!(event = "S3_UPLOAD_ERROR", error = ?e, "❌ S3 Upload {} deneme sonrası başarısız oldu.", MAX_RETRIES);
                    return Err(anyhow::anyhow!("S3 Upload Bounded Retry Failed: {:?}", e));
                }
                
                let backoff = Duration::from_millis(2u64.pow(attempt) * 500);
                warn!(event = "S3_UPLOAD_RETRY", attempt = attempt, backoff_ms = backoff.as_millis(), "⚠️ S3 Upload başarısız, tekrar deneniyor...");
                sleep(backoff).await;
            }
            Err(_) => {
                // Timeout Triggered
                counter!(S3_UPLOAD_FAILURES).increment(1);
                
                if attempt >= MAX_RETRIES {
                    error!(event = "S3_UPLOAD_TIMEOUT_FATAL", "❌ S3 Upload {} deneme boyunca Timeout yedi.", MAX_RETRIES);
                    return Err(anyhow::anyhow!("S3 Upload Timeout Exceeded after retries"));
                }
                
                let backoff = Duration::from_millis(2u64.pow(attempt) * 500);
                warn!(event = "S3_UPLOAD_TIMEOUT", attempt = attempt, "⚠️ S3 Upload isteği 15 saniyede cevap vermedi (Timeout). Tekrar deneniyor...");
                sleep(backoff).await;
            }
        }
    }
}