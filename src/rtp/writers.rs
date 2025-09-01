// File: src/rtp/writers.rs (GÜNCELLENDİ)
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client as S3Client; // DEĞİŞİKLİK: Client'ı Arc içinde tutacağız
use std::path::Path;
use std::sync::Arc; // DEĞİŞİKLİK
use tracing::info;
use url::Url;

use crate::config::AppConfig;
use crate::state::AppState; // YENİ

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
    // DEĞİŞİKLİK: Artık S3 client'ını Arc ile paylaşıyoruz.
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
        info!("Kayıt dosyası başarıyla S3 bucket'ına yazıldı.");
        Ok(())
    }
}

// DEĞİŞİKLİK: Fonksiyon artık AppConfig yerine AppState alıyor.
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
            
            // YENİ: S3 istemcisini oluşturmak yerine AppState'ten alıyoruz.
            let client = app_state.s3_client.clone().ok_or_else(|| {
                anyhow!("S3 URI'si kullanıldı ancak paylaşılan S3 istemcisi başlatılamamış.")
            })?;

            let bucket = s3_config.bucket_name.clone();
            let key = uri.path().trim_start_matches('/').to_string();

            if key.is_empty() {
                return Err(anyhow!("S3 URI'sinde dosya yolu (key) belirtilmelidir."));
            }

            Ok(Box::new(S3Writer { client, bucket, key }))
        }
        scheme => Err(anyhow!("Desteklenmeyen kayıt URI şeması: {}", scheme)),
    }
}