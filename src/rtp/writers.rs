// File: src/rtp/writers.rs (TAM VE HATASIZ NİHAİ HALİ)
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client as S3Client;
use std::path::Path;
use std::sync::Arc;
use tracing::info;
use url::Url;

use crate::config::AppConfig;
use crate::state::AppState;

#[async_trait]
pub trait AsyncRecordingWriter: Send + Sync {
    async fn write(&self, data: Vec<u8>) -> Result<()>;
}

struct FileWriter {
    path: String,
}

#[async_trait]
impl AsyncRecordingWriter for FileWriter {
    async fn write(&self, data: Vec<u8>) -> Result<()> {
        let path = Path::new(&self.path);
        if let Some(parent_dir) = path.parent() {
            tokio::fs::create_dir_all(parent_dir).await?;
        }
        tokio::fs::write(path, data).await?;
        info!("Kayıt dosyası başarıyla diske yazıldı.");
        Ok(())
    }
}

struct S3Writer {
    client: Arc<S3Client>,
    bucket: String,
    key: String,
}

#[async_trait]
impl AsyncRecordingWriter for S3Writer {
    async fn write(&self, data: Vec<u8>) -> Result<()> {
        let body = ByteStream::from(data);
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(&self.key)
            .body(body)
            .send()
            .await
            .context("S3'ye obje yüklenemedi")?;
        info!(bucket = %self.bucket, key = %self.key, "Kayıt dosyası başarıyla S3 bucket'ına yazıldı.");
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
        "file" => {
            let path = uri.to_file_path().map_err(|_| anyhow!("Geçersiz dosya yolu"))?;
            Ok(Box::new(FileWriter { path: path.to_string_lossy().to_string() }))
        }
        "s3" => {
            let s3_config = config.s3_config.as_ref().ok_or_else(|| {
                anyhow!("S3 URI'si belirtildi ancak S3 konfigürasyonu ortamda bulunamadı.")
            })?;
            
            let client = app_state.s3_client.clone().ok_or_else(|| {
                anyhow!("S3 URI'si kullanıldı ancak paylaşılan S3 istemcisi başlatılamamış.")
            })?;

            // --- DEĞİŞİKLİK BURADA ---
            // ÖNCEKİ YANLIŞ HALİ:
            // let key = format!("{}{}", uri.host_str().unwrap_or(""), uri.path()).trim_start_matches('/').to_string();
            
            // YENİ DOĞRU HALİ:
            // S3 anahtarı, URI'nin sadece path kısmıdır. Host kısmı bucket'ı temsil eder.
            // SDK'ya bucket'ı ayrı verdiğimiz için anahtara dahil etmemeliyiz.
            let bucket = uri.host_str().unwrap_or(&s3_config.bucket_name).to_string();
            let key = uri.path().trim_start_matches('/').to_string();
            // --- DEĞİŞİKLİK SONU ---

            if key.is_empty() {
                return Err(anyhow!("S3 URI'sinde dosya yolu (key) belirtilmelidir."));
            }

            Ok(Box::new(S3Writer { client, bucket, key }))
        }
        scheme => Err(anyhow!("Desteklenmeyen kayıt URI şeması: {}", scheme)),
    }
}