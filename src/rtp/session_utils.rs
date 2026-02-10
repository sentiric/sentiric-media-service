// sentiric-media-service/src/rtp/session_utils.rs
use super::command::RecordingSession;
use crate::audio::load_or_get_from_cache;
use crate::config::AppConfig;
use crate::rabbitmq;
use crate::rtp::writers;
use crate::state::AppState;

use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose, Engine};
use hound::WavWriter;
use lapin::{options::BasicPublishOptions, BasicProperties};
// DÜZELTME: rubato kaldırıldı, simple_resample eklendi
use sentiric_rtp_core::simple_resample; 
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::task::spawn_blocking;
use tracing::{instrument, warn};

#[instrument(skip_all, fields(uri = %session.output_uri, call_id = %session.call_id))]
pub async fn finalize_and_save_recording(session: RecordingSession, app_state: AppState) -> Result<()> {
    if session.mixed_samples_16khz.is_empty() {
        warn!("Kaydedilecek ses verisi yok.");
        return Ok(());
    }

    let call_id = session.call_id.clone();
    let output_uri = session.output_uri.clone();

    let mixed_samples = session.mixed_samples_16khz;
    let spec = session.spec;

    // Resampling (16k -> 8k) - Artık Iron Core kullanıyor
    let downsampled = spawn_blocking(move || -> Result<Vec<i16>> {
        // Cubic interpolation ile temiz ve hızlı dönüşüm
        Ok(simple_resample(&mixed_samples, 16000, 8000))
    }).await.context("Resampling task failed")??;

    let wav_data = spawn_blocking(move || -> Result<Vec<u8>> {
        let mut buffer = Cursor::new(Vec::new());
        let mut writer = WavWriter::new(&mut buffer, spec)?;
        for sample in downsampled { writer.write_sample(sample)?; }
        writer.finalize()?;
        Ok(buffer.into_inner())
    }).await.context("WAV encode failed")??;

    let writer = writers::from_uri(&output_uri, &app_state, &app_state.port_manager.config).await?;
    writer.write(wav_data).await.map_err(|e| anyhow!(e))?; 

    if let Some(publ) = &app_state.rabbitmq_publisher {
        let payload = serde_json::json!({ "callId": call_id, "uri": output_uri }).to_string();
        let _ = publ.basic_publish(rabbitmq::EXCHANGE_NAME, "call.recording.available", 
            BasicPublishOptions::default(), payload.as_bytes(), BasicProperties::default()).await;
    }

    Ok(())
}

pub async fn load_and_resample_samples_from_uri(
    uri: &str,
    app_state: &AppState,
    config: &Arc<AppConfig>,
) -> Result<Arc<Vec<i16>>> {
    if let Some(path_part) = uri.strip_prefix("file://") {
        let mut final_path = PathBuf::from(&config.assets_base_path);
        final_path.push(path_part.trim_start_matches('/'));

        let samples_8k = load_or_get_from_cache(&app_state.audio_cache, &final_path).await?;
        
        let samples_16k = spawn_blocking(move || {
            // 8k -> 16k Upsample (Iron Core)
            simple_resample(&samples_8k, 8000, 16000)
        }).await?;
        
        return Ok(Arc::new(samples_16k));
    }
    
    if uri.starts_with("data:") {
        let base64_str = uri.split(",").nth(1).ok_or_else(|| anyhow!("Invalid data URI"))?;
        let bytes = general_purpose::STANDARD.decode(base64_str)?;
        let samples: Vec<i16> = bytes.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
        return Ok(Arc::new(samples));
    }

    Err(anyhow!("Unsupported URI scheme"))
}