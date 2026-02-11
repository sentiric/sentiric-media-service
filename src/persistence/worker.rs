// sentiric-media-service/src/persistence/worker.rs
use std::path::PathBuf;
use tokio::fs;
use tokio::time::{sleep, Duration};
use crate::state::AppState;
use crate::rtp::writers;
use crate::rabbitmq;
use lapin::{options::BasicPublishOptions, BasicProperties};
use serde_json;

pub struct UploadWorker {
    app_state: AppState,
    staging_dir: PathBuf,
}

impl UploadWorker {
    pub fn new(app_state: App_State) -> Self {
        Self { 
            app_state, 
            staging_dir: PathBuf::from("/tmp/sentiric/recordings") 
        }
    }

    pub async fn run(self) {
        let _ = fs::create_dir_all(&self.staging_dir).await;
        tracing::info!("🚀 S3 Upload Worker active. Target: {:?}", self.staging_dir);

        loop {
            if let Ok(mut entries) = fs::read_dir(&self.staging_dir).await {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    let path = entry.path();
                    // Sadece .wav dosyalarını işle (.tmp olanları bekle)
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
        if parts.len() < 1 { return; }
        
        let call_id = parts[0];
        let s3_uri = format!("s3://sentiric/recordings/{}.wav", call_id);

        match fs::read(&path).await {
            Ok(data) => {
                let writer_res = writers::from_uri(&s3_uri, &self.app_state, &self.app_state.port_manager.config).await;
                match writer_res {
                    Ok(writer) => {
                        if writer.write(data).await.is_ok() {
                            tracing::info!("✅ S3 Persistence Success: {}", call_id);
                            let _ = self.notify_rabbitmq(call_id, &s3_uri).await;
                            let _ = fs::remove_file(path).await;
                        }
                    },
                    Err(e) => tracing::error!("Writer selection error: {}", e),
                }
            }
            Err(e) => tracing::error!("Failed to read staged file: {}", e),
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
        }
        Ok(())
    }
}

// AppState tipini persistence içinde kullanabilmek için takma ad
use crate::state::AppState as App_State;