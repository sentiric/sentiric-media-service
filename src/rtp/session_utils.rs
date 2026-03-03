// src/rtp/session_utils.rs
use super::command::RecordingSession;
use crate::state::AppState;
use anyhow::Result;
use hound::{WavWriter, WavSpec, SampleFormat}; // Spec eklendi
use std::io::Cursor;
use tokio::fs;
use tokio::task::spawn_blocking;
use tracing::{instrument, info}; // Info eklendi

#[instrument(skip_all, fields(call_id = %session.call_id))]
pub async fn finalize_and_save_recording(session: RecordingSession, app_state: AppState) -> Result<()> {
    if session.audio_buffer.is_empty() {
        return Ok(());
    }

    let recordings_dir = &app_state.port_manager.config.media_recording_path;
    let staging_path = format!("{}/{}_{}.wav", recordings_dir, session.call_id, session.trace_id);
    let tmp_path = format!("{}.tmp", staging_path);

    // [TELECOM STANDARD]: Kayıt formatını 8000Hz Mono olarak zorluyoruz.
    let spec = WavSpec {
        channels: 1,
        sample_rate: 8000,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    
    let samples = session.audio_buffer;
    
    let wav_data = spawn_blocking(move || -> Result<Vec<u8>> {
        let mut buffer = Cursor::new(Vec::new());
        let mut writer = WavWriter::new(&mut buffer, spec)?;
        
        // [DIRECT WRITE]: Artık downsample yok. Veri zaten native 8kHz.
        for sample in samples {
            writer.write_sample(sample)?;
        }
        
        writer.finalize()?;
        Ok(buffer.into_inner())
    }).await??;

    fs::create_dir_all(recordings_dir).await?;
    fs::write(&tmp_path, wav_data).await?;
    fs::rename(&tmp_path, &staging_path).await?; 

    info!(path = %staging_path, "💾 Kayıt başarıyla tamamlandı (Native 8kHz).");
    Ok(())
}

// ... (load_and_resample fonksiyonu aynı kalabilir)
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
        
        // Burası TTS/Playback için. Buradaki 16k upsample kalabilir çünkü
        // encode fonksiyonu 16k bekleyip 8k'ya düşürüyor (codec uyumu için).
        let samples_16k = tokio::task::spawn_blocking(move || {
            sentiric_rtp_core::simple_resample(&samples_8k, 8000, 16000)
        }).await?;
        return Ok(std::sync::Arc::new(samples_16k));
    }
    Err(anyhow::anyhow!("Desteklenmeyen URI şeması: {}", uri))
}