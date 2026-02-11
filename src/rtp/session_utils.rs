// sentiric-media-service/src/rtp/session_utils.rs
use super::command::RecordingSession;
use crate::state::AppState;
use anyhow::Result;
use hound::WavWriter;
use std::io::Cursor;
use tokio::fs;
use tokio::task::spawn_blocking;
use tracing::instrument;

#[instrument(skip_all, fields(call_id = %session.call_id))]
pub async fn finalize_and_save_recording(session: RecordingSession, _app_state: AppState) -> Result<()> {
    if session.mixed_samples_16khz.is_empty() {
        return Ok(());
    }

    let recordings_dir = "/tmp/sentiric/recordings";
    let staging_path = format!("{}/{}_{}.wav", recordings_dir, session.call_id, session.trace_id);
    let tmp_path = format!("{}.tmp", staging_path);

    // 1. WAV Encode (Sync Task)
    let spec = session.spec;
    let mixed_samples = session.mixed_samples_16khz;
    
    let wav_data = spawn_blocking(move || -> Result<Vec<u8>> {
        let mut buffer = Cursor::new(Vec::new());
        let mut writer = WavWriter::new(&mut buffer, spec)?;
        
        // 16k -> 8k Downsampling (rtp-core kütüphanesinden)
        let downsampled = sentiric_rtp_core::simple_resample(&mixed_samples, 16000, 8000);
        
        for sample in downsampled {
            writer.write_sample(sample)?;
        }
        writer.finalize()?;
        Ok(buffer.into_inner())
    }).await??;

    // 2. Diske Atomic Yazım
    fs::create_dir_all(recordings_dir).await?;
    fs::write(&tmp_path, wav_data).await?;
    fs::rename(&tmp_path, &staging_path).await?; // Atomic rename

    tracing::info!(path = %staging_path, "💾 Recording staged for background upload.");
    Ok(())
}

// Diğer kullanılmayan loader fonksiyonu (opsiyonel tutuldu)
pub async fn load_and_resample_samples_from_uri(
    uri: &str,
    app_state: &AppState,
    config: &std::sync::Arc<crate::config::AppConfig>,
) -> Result<std::sync::Arc<Vec<i16>>> {
    use crate::audio::load_or_get_from_cache;
    use std::path::PathBuf;

    if let Some(path_part) = uri.strip_prefix("file://") {
        let mut final_path = PathBuf::from(&config.assets_base_path);
        final_path.push(path_part.trim_start_matches('/'));
        let samples_8k = load_or_get_from_cache(&app_state.audio_cache, &final_path).await?;
        let samples_16k = spawn_blocking(move || {
            sentiric_rtp_core::simple_resample(&samples_8k, 8000, 16000)
        }).await?;
        return Ok(std::sync::Arc::new(samples_16k));
    }
    Err(anyhow::anyhow!("Unsupported URI scheme"))
}