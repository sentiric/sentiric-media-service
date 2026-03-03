// sentiric-media-service/src/persistence/worker.rs
use std::path::PathBuf;
use tokio::fs;
use tokio::time::{sleep, Duration};
use crate::state::AppState;
use crate::rtp::writers;
use crate::rabbitmq;
use lapin::{options::BasicPublishOptions, BasicProperties};
use serde_json;
use tracing::{info, error, warn, debug};

pub struct UploadWorker {
    app_state: AppState,
    staging_dir: PathBuf,
}

impl UploadWorker {
    pub fn new(app_state: App_State) -> Self {
        let path = app_state.port_manager.config.media_recording_path.clone();
        Self { 
            app_state, 
            staging_dir: PathBuf::from(path) 
        }
    }

    pub async fn run(self) {
        let _ = fs::create_dir_all(&self.staging_dir).await;
        info!(
            event = "WORKER_STARTED", 
            target_dir = %self.staging_dir.display(),
            "🚀 S3 Upload Worker aktif. Klasör izleniyor..."
        );

        loop {
            if let Ok(mut entries) = fs::read_dir(&self.staging_dir).await {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    let path = entry.path();
                    if path.extension().and_then(|s| s.to_str()) == Some("wav") {
                        self.process_file(path).await;
                    }
                }
            }
            sleep(Duration::from_secs(5)).await;
        }
    }

    async fn process_file(&self, path: PathBuf) {
        let file_name = path.file_name().unwrap().to_str().unwrap().to_string();
        let parts: Vec<&str> = file_name.trim_end_matches(".wav").split('_').collect();
        if parts.is_empty() { return; }
        
        let call_id = parts[0];
        let s3_uri = format!("s3://sentiric/recordings/{}.wav", call_id);

        debug!(event = "WORKER_FILE_FOUND", file = %file_name, "İşlenecek dosya bulundu.");

        match fs::read(&path).await {
            Ok(data) => {
                let writer_res = writers::from_uri(&s3_uri, &self.app_state, &self.app_state.port_manager.config).await;
                match writer_res {
                    Ok(writer) => {
                        // Yazma işlemini dene
                        if let Err(e) = writer.write(data).await {
                            // [KRİTİK DÜZELTME]: S3 hatası aldıysak sonsuz döngüyü kırmak için dosyayı .failed yap.
                            error!(event = "WORKER_S3_ERROR", error = %e, file = %file_name, "❌ S3'e yazılamadı! Dosya '.failed' olarak işaretleniyor.");
                            let failed_path = path.with_extension("failed");
                            let _ = fs::rename(&path, &failed_path).await;
                        } else {
                            // Başarılı
                            info!(event = "WORKER_SUCCESS", call_id = %call_id, s3_uri = %s3_uri, "✅ Dosya başarıyla aktarıldı ve siliniyor.");
                            let _ = self.notify_rabbitmq(call_id, &s3_uri).await;
                            let _ = fs::remove_file(path).await;
                        }
                    },
                    Err(e) => {
                        error!(event = "WORKER_WRITER_ERROR", error = %e, "S3 Writer oluşturulamadı. Ayarları kontrol edin.");
                        let failed_path = path.with_extension("failed");
                        let _ = fs::rename(&path, &failed_path).await;
                    },
                }
            }
            Err(e) => error!(event = "WORKER_READ_ERROR", error = %e, "Dosya diskten okunamadı!"),
        }
    }

    async fn notify_rabbitmq(&self, call_id: &str, uri: &str) -> anyhow::Result<()> {
        if let Some(chan) = &self.app_state.rabbitmq_publisher {
            let payload = serde_json::json!({ "callId": call_id, "uri": uri }).to_string();
            chan.basic_publish(
                rabbitmq::EXCHANGE_NAME,
                "call.recording.available",
                BasicPublishOptions::default(),
                payload.as_bytes(),
                BasicProperties::default(),
            ).await?;
        } else {
            warn!(event = "WORKER_MQ_WARN", "RabbitMQ Publisher kapalı, event atılamadı.");
        }
        Ok(())
    }
}

use crate::state::AppState as App_State;