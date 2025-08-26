// src/rtp/writers.rs (EN HAFİF SÜRÜM) ###

use anyhow::{anyhow, Result};
use std::path::Path;
use async_trait::async_trait;
use tracing::{info, instrument, warn};

#[async_trait]
pub trait AsyncRecordingWriter: Send + Sync {
    async fn write(&self, data: Vec<u8>) -> Result<()>;
}

struct FileWriter {
    path: String,
}

#[async_trait]
impl AsyncRecordingWriter for FileWriter {
    #[instrument(skip(self, data), fields(path = %self.path))]
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

// from_uri fonksiyonu artık sadece file:// şemasını tanır.
pub fn from_uri(uri_str: &str) -> Result<Box<dyn AsyncRecordingWriter>> {
    if let Some(path_part) = uri_str.strip_prefix("file://") {
        Ok(Box::new(FileWriter { path: path_part.to_string() }))
    } else {
        // s3:// gibi bilinmeyen bir şema gelirse, bunu loglayıp hata dönelim.
        warn!("Desteklenmeyen kayıt URI şeması alındı: {}", uri_str);
        Err(anyhow!("Desteklenmeyen kayıt URI şeması: {}. Şimdilik sadece 'file://' desteklenmektedir.", uri_str))
    }
}