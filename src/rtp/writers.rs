// sentiric-media-service/src/rtp/writers.rs

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client as S3Client;
use std::sync::Arc;
use tracing::{debug, info, instrument};
use url::Url;

use crate::config::AppConfig;
use crate::state::AppState;

#[async_trait]
pub trait AsyncRecordingWriter: Send + Sync {
    async fn write(&self, data: Vec<u8>) -> Result<()>;
}

// DÜZELTME: Kullanılmayan FileWriter struct'ı ve ilgili kod kaldırıldı.
// struct FileWriter {
//     path: String,
// }
// #[async_trait]
// impl AsyncRecordingWriter for FileWriter { ... }

struct S3Writer {
    client: Arc<S3Client>,
    bucket: String,
    key: String,
}

#[async_trait]
impl AsyncRecordingWriter for S3Writer {
    #[instrument(skip(self, data), fields(s3.bucket = %self.bucket, s3.key = %self.key, s3.data_size_kb = data.len() / 1024))]
    async fn write(&self, data: Vec<u8>) -> Result<()> {
        let body = ByteStream::from(data);
        info!("Kayıt dosyası S3 bucket'ına yükleniyor...");
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(&self.key)
            .body(body)
            .send()
            .await
            .context("S3'ye obje yüklenemedi")?;
        Ok(())
    }
}

pub async fn from_uri(
    uri_str: &str,
    app_state: &AppState,
    config: &AppConfig,
) -> Result<Box<dyn AsyncRecordingWriter>> {
    let uri = Url::parse(uri_str).context("Geçersiz kayıt URI formatı")?;

    match uri.scheme() {
        // NOT: 'file' şeması şu anda desteklenmemektedir. Gelecekte yerel dosya
        // sistemine kayıt yapma ihtiyacı doğarsa, 'FileWriter' struct'ı ve ilgili
        // mantık buraya yeniden eklenebilir. Mevcut mimari S3 uyumlu hedeflere odaklanmıştır.
        
        "s3" => {
            let s3_config = config.s3_config.as_ref().ok_or_else(|| {
                anyhow!("S3 URI'si belirtildi ancak S3 konfigürasyonu ortamda bulunamadı.")
            })?;
            
            let client = app_state.s3_client.clone().ok_or_else(|| {
                anyhow!("S3 URI'si kullanıldı ancak paylaşılan S3 istemcisi başlatılamamış.")
            })?;

            let bucket = uri.host_str().unwrap_or(&s3_config.bucket_name).to_string();
            let key = uri.path().trim_start_matches('/').to_string();

            if key.is_empty() {
                return Err(anyhow!("S3 URI'sinde dosya yolu (key) belirtilmelidir."));
            }

            debug!(
                s3.provider = "minio",
                s3.endpoint = %s3_config.endpoint_url,
                s3.bucket = %bucket,
                s3.key = %key,
                "S3 yazıcısı (writer) oluşturuldu."
            );

            Ok(Box::new(S3Writer { client, bucket, key }))
        }
        scheme => Err(anyhow!("Desteklenmeyen kayıt URI şeması: {}", scheme)),
    }
}