// File: src/rtp/writers.rs

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use aws_config::meta::region::RegionProviderChain;
use aws_config::{BehaviorVersion, Region};
use aws_credential_types::Credentials;
use aws_sdk_s3::config::Builder as S3ConfigBuilder;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client as S3Client;
use std::path::Path;
use tracing::info;
use url::Url;

use crate::config::AppConfig;

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
    client: S3Client,
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

pub async fn from_uri(
    uri_str: &str,
    config: &AppConfig,
) -> Result<Box<dyn AsyncRecordingWriter>> {
    let uri = Url::parse(uri_str).context("Geçersiz kayıt URI formatı")?;

    match uri.scheme() {
        "file" => {
            let path = uri
                .to_file_path()
                .map_err(|_| anyhow!("Geçersiz dosya yolu"))?;
            Ok(Box::new(FileWriter {
                path: path.to_string_lossy().to_string(),
            }))
        }
        "s3" => {
            let s3_config = config.s3_config.as_ref().ok_or_else(|| {
                anyhow!("S3 URI'si belirtildi ancak S3 konfigürasyonu ortamda bulunamadı.")
            })?;

            let bucket = s3_config.bucket_name.clone();
            let key = uri.path().trim_start_matches('/').to_string();

            if key.is_empty() {
                return Err(anyhow!("S3 URI'sinde dosya yolu (key) belirtilmelidir."));
            }

            let region_provider = RegionProviderChain::first_try(Region::new(s3_config.region.clone()));

            let sdk_config = aws_config::defaults(BehaviorVersion::latest())
                .region(region_provider)
                .endpoint_url(&s3_config.endpoint_url)
                .credentials_provider(Credentials::new(
                    &s3_config.access_key_id,
                    &s3_config.secret_access_key,
                    None,
                    None,
                    "Static",
                ))
                .load()
                .await;
            
            // MinIO ve R2 gibi S3 uyumlu sistemler için yol tabanlı adreslemeyi zorunlu kıl
            let s3_client_config = S3ConfigBuilder::from(&sdk_config)
                .force_path_style(true)
                .build();
            let client = S3Client::from_conf(s3_client_config);

            Ok(Box::new(S3Writer {
                client,
                bucket,
                key,
            }))
        }
        scheme => Err(anyhow!("Desteklenmeyen kayıt URI şeması: {}", scheme)),
    }
}