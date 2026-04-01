// sentiric-media-service/src/rtp/session_utils.rs
use super::command::RecordingSession;
use crate::metrics::RECORDING_BUFFER_BYTES;
use crate::rabbitmq;
use crate::rtp::writers;
use crate::state::AppState;
use anyhow::{anyhow, Result};
use chrono::Datelike;
use hound::{SampleFormat, WavSpec, WavWriter};
use metrics::gauge;
use prost::Message;
use std::io::Cursor;
use tokio::task::spawn_blocking;
use tracing::{error, info, instrument};

use sentiric_contracts::sentiric::event::v1::CallRecordingAvailableEvent;

// [ARCH-COMPLIANCE] upload_to_s3 ve RabbitMQ event loglarındaki eksik bağlam onarılmıştır.
#[instrument(skip_all, fields(call_id = %session.call_id))]
pub async fn finalize_and_save_recording(
    session: RecordingSession,
    app_state: AppState,
) -> Result<()> {
    let rx_len = session.rx_buffer.len();
    let tx_len = session.tx_buffer.len();
    let max_len = rx_len.max(tx_len);

    gauge!(RECORDING_BUFFER_BYTES).decrement(((rx_len + tx_len) * 2) as f64);

    if max_len == 0 {
        // [ARCH-COMPLIANCE] sip.call_id eklendi
        info!(event = "RECORDING_SKIPPED", sip.call_id = %session.call_id, "Boş kayıt, işlem atlanıyor.");
        return Ok(());
    }

    let now = chrono::Utc::now();
    let s3_key = format!(
        "recordings/{}/{:02}/{:02}/{}.wav",
        now.year(),
        now.month(),
        now.day(),
        session.call_id
    );

    let spec = WavSpec {
        channels: 2,
        sample_rate: 8000,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };

    let rx_buffer = session.rx_buffer;
    let tx_buffer = session.tx_buffer;

    let wav_data = spawn_blocking(move || -> Result<Vec<u8>> {
        let mut buffer = Cursor::new(Vec::with_capacity(max_len * 4 + 44));
        let mut writer = WavWriter::new(&mut buffer, spec)?;

        for i in 0..max_len {
            let left = if i < rx_buffer.len() { rx_buffer[i] } else { 0 };
            let right = if i < tx_buffer.len() { tx_buffer[i] } else { 0 };
            writer.write_sample(left)?;
            writer.write_sample(right)?;
        }

        writer.finalize()?;
        Ok(buffer.into_inner())
    })
    .await??;

    let s3_config = app_state
        .port_manager
        .config
        .s3_config
        .clone()
        .ok_or_else(|| anyhow!("S3 config eksik"))?;
    let s3_client = app_state
        .s3_client
        .clone()
        .ok_or_else(|| anyhow!("S3 client eksik"))?;

    // [ARCH-COMPLIANCE] Fonksiyon çağrısına session.call_id paslandı
    writers::upload_to_s3_with_retry(
        s3_client,
        &s3_config.bucket_name,
        &s3_key,
        wav_data,
        &session.call_id,
    )
    .await?;

    if let Some(channel) = &app_state.rabbitmq_publisher {
        let s3_uri = format!("s3://{}/{}", s3_config.bucket_name, s3_key);
        let event = CallRecordingAvailableEvent {
            event_type: "call.recording.available".to_string(),
            trace_id: session.trace_id,
            call_id: session.call_id.clone(),
            timestamp: Some(prost_types::Timestamp::from(std::time::SystemTime::now())),
            recording_uri: s3_uri,
            public_url: "".to_string(),
        };

        match rabbitmq::publish_with_confirm(
            channel,
            "call.recording.available",
            &event.encode_to_vec(),
        )
        .await
        {
            //[ARCH-COMPLIANCE] sip.call_id loga eklendi.
            Ok(_) => {
                info!(event = "RECORDING_EVENT_PUBLISHED", sip.call_id = %session.call_id, "📩 Stereo kayıt tamamlandı olayı (Confirmed) RabbitMQ'ya iletildi.")
            }
            Err(e) => {
                error!(event = "RECORDING_EVENT_PUBLISH_FAIL", sip.call_id = %session.call_id, error = %e, "🔥 Kayıt S3'e yüklendi ama RabbitMQ'ya olay atılamadı!");
                return Err(e.into());
            }
        }
    }

    Ok(())
}

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
        return Ok(samples_8k);
    }
    Err(anyhow!("Desteklenmeyen URI şeması: {}", uri))
}
