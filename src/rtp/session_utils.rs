// sentiric-media-service/src/rtp/session_utils.rs

use super::command::RecordingSession;
use crate::audio::load_or_get_from_cache;
use crate::config::AppConfig;
use crate::rabbitmq;
use crate::rtp::writers;
use crate::state::AppState;
use crate::rtp::stream::decode_audio_with_symphonia;
use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose, Engine};
use hound::WavWriter;
use lapin::{options::BasicPublishOptions, BasicProperties};
use rubato::{Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::task::spawn_blocking;
use tracing::{error, info, instrument, warn};

// ... (finalize_and_save_recording AYNI KALSIN) ...
#[instrument(skip_all, fields(uri = %session.output_uri, call_id = %session.call_id, trace_id = %session.trace_id))]
pub async fn finalize_and_save_recording(session: RecordingSession, app_state: AppState) -> Result<()> {
    if session.mixed_samples_16khz.is_empty() {
        warn!("Kaydedilecek ses verisi yok, boş dosya oluşturulmayacak.");
        return Ok(());
    }

    info!(mixed_samples = session.mixed_samples_16khz.len(), "Kayıt sonlandırılıyor.");

    let call_id_for_event = session.call_id.clone();
    let trace_id_for_event = session.trace_id.clone();
    let output_uri_for_event = session.output_uri.clone();

    let result: Result<()> = async {
        let mixed_samples_16k = session.mixed_samples_16khz;
        let spec = session.spec;

        let downsampled_samples_8k = spawn_blocking(move || -> Result<Vec<i16>> {
            let pcm_f32: Vec<f32> = mixed_samples_16k.iter().map(|s| *s as f32 / 32768.0).collect();
            let params = SincInterpolationParameters {
                sinc_len: 256,
                f_cutoff: 0.95,
                interpolation: SincInterpolationType::Linear,
                oversampling_factor: 256,
                window: WindowFunction::BlackmanHarris2,
            };
            let mut resampler = SincFixedIn::<f32>::new(
                8000.0 / 16000.0,
                2.0,
                params,
                pcm_f32.len(),
                1,
            )?;
            let mut resampled_channels = resampler.process(&[pcm_f32], None)?;
            if resampled_channels.is_empty() {
                return Err(anyhow!("Resampler ses kanalı döndürmedi."));
            }
            Ok(resampled_channels
                .remove(0)
                .into_iter()
                .map(|s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16)
                .collect())
        })
        .await
        .context("8kHz'e düşürme task'i başarısız oldu")??;

        let mut spec_8k = spec;
        spec_8k.sample_rate = 8000;

        let wav_data = spawn_blocking(move || -> Result<Vec<u8>, hound::Error> {
            let mut buffer = Cursor::new(Vec::new());
            let mut writer = WavWriter::new(&mut buffer, spec_8k)?;
            for sample in downsampled_samples_8k {
                writer.write_sample(sample)?;
            }
            writer.finalize()?;
            Ok(buffer.into_inner())
        })
        .await
        .context("WAV dosyası oluşturma task'i başarısız oldu")??;

        info!(bytes = wav_data.len(), "WAV verisi başarıyla oluşturuldu, hedefe yazılıyor.");

        let writer = writers::from_uri(
            &output_uri_for_event,
            &app_state,
            &app_state.port_manager.config,
        )
        .await
        .context("Kayıt yazıcısı oluşturulamadı")?;

        writer
            .write(wav_data)
            .await
            .map_err(|e| {
                error!(source_error = ?e, "Kayıt verisi hedefe yazılamadı.");
                anyhow::anyhow!(e).context("Veri yazılamadı")
            })?;

        Ok(())
    }
    .await;

    if result.is_ok() {
        if let Some(publisher) = &app_state.rabbitmq_publisher {
            let event_payload = serde_json::json!({
                "eventType": "call.recording.available",
                "traceId": trace_id_for_event,
                "callId": call_id_for_event,
                "recordingUri": output_uri_for_event,
                "timestamp": chrono::Utc::now().to_rfc3339()
            });
            if let Err(e) = publisher
                .basic_publish(
                    rabbitmq::EXCHANGE_NAME,
                    "call.recording.available",
                    BasicPublishOptions::default(),
                    event_payload.to_string().as_bytes(),
                    BasicProperties::default().with_delivery_mode(2),
                )
                .await
            {
                error!(error = ?e, "Kayıt olayı yayınlanamadı.");
            } else {
                info!("'call.recording.available' olayı başarıyla yayınlandı.");
            }
        }
    }
    result
}

// --- DÜZELTİLEN FONKSİYON ---
pub async fn load_and_resample_samples_from_uri(
    uri: &str,
    app_state: &AppState,
    config: &Arc<AppConfig>,
) -> Result<Arc<Vec<i16>>> {
    if uri.starts_with("data:") {
        info!("Data URI'sinden ses verisi alınıyor.");
        
        // Base64 verisini daha güvenli ayıkla
        // "data:audio/pcm;base64," kısmından sonrasını al
        let base64_data = if let Some(idx) = uri.find(";base64,") {
            &uri[idx + 8..]
        } else {
            return Err(anyhow!("Geçersiz data URI formatı: base64 etiketi bulunamadı"));
        };
            
        // Boşlukları ve yeni satırları temizle (Robustness)
        let clean_base64 = base64_data.replace(|c: char| c.is_whitespace(), "");

        let audio_bytes = general_purpose::STANDARD
            .decode(&clean_base64)
            .context("Base64 verisi çözümlenemedi")?;

        // RAW PCM (8kHz, 16-bit, Mono) varsayımı
        // WAV Header kontrolü
        let raw_samples_bytes = if audio_bytes.len() > 44 && &audio_bytes[0..4] == b"RIFF" {
            info!("WAV Header tespit edildi, header atlanarak raw data okunuyor.");
            &audio_bytes[44..]
        } else {
            &audio_bytes[..]
        };
        
        let samples_8k: Vec<i16> = raw_samples_bytes
            .chunks_exact(2)
            .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();
            
        if samples_8k.is_empty() {
             return Err(anyhow!("Data URI ses verisi boş."));
        }

        // 8kHz -> 16kHz Resampling
        let resampled_samples_16khz = spawn_blocking(move || -> Result<Vec<i16>> {
            info!(samples = samples_8k.len(), "Data URI sesi 16kHz'e yükseltiliyor.");
            
            let pcm_f32: Vec<f32> = samples_8k.iter().map(|s| *s as f32 / 32768.0).collect();
            
            let params = SincInterpolationParameters {
                sinc_len: 64,
                f_cutoff: 0.95,
                interpolation: SincInterpolationType::Linear,
                oversampling_factor: 128,
                window: WindowFunction::BlackmanHarris2,
            };
            
            let mut resampler = SincFixedIn::<f32>::new(2.0, 2.0, params, pcm_f32.len(), 1)?;
            let mut resampled_channels = resampler.process(&[pcm_f32], None)?;
            
            if resampled_channels.is_empty() {
                return Err(anyhow!("Resampler ses kanalı döndürmedi."));
            }
            
            Ok(resampled_channels.remove(0).into_iter()
                .map(|s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16)
                .collect())
        }).await??;

        return Ok(Arc::new(resampled_samples_16khz));
    }

    if let Some(path_part) = uri.strip_prefix("file://") {
        let mut final_path = PathBuf::from(&config.assets_base_path);
        final_path.push(path_part.trim_start_matches('/'));

        let samples_from_file_8khz =
            load_or_get_from_cache(&app_state.audio_cache, &final_path).await?;

        let resampled_samples_16khz = spawn_blocking(move || -> Result<Vec<i16>> {
            info!(
                samples = samples_from_file_8khz.len(),
                "Dosyadan okunan ses 16kHz'e yeniden örnekleniyor."
            );
            let pcm_f32: Vec<f32> = samples_from_file_8khz
                .iter()
                .map(|s| *s as f32 / 32768.0)
                .collect();
            let params = SincInterpolationParameters {
                sinc_len: 256,
                f_cutoff: 0.95,
                interpolation: SincInterpolationType::Linear,
                oversampling_factor: 256,
                window: WindowFunction::BlackmanHarris2,
            };
            let mut resampler =
                SincFixedIn::<f32>::new(16000.0 / 8000.0, 2.0, params, pcm_f32.len(), 1)?;
            let mut resampled_channels = resampler.process(&[pcm_f32], None)?;
            if resampled_channels.is_empty() {
                return Err(anyhow!("Resampler ses kanalı döndürmedi."));
            }
            Ok(resampled_channels
                .remove(0)
                .into_iter()
                .map(|s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16)
                .collect())
        })
        .await
        .context("Anonsu 16kHz'e yükseltme task'i başarısız oldu")??;

        return Ok(Arc::new(resampled_samples_16khz));
    }

    Err(anyhow!("Desteklenmeyen URI şeması: {}", uri))
}