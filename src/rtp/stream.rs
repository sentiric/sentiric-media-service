use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use anyhow::{Result, Context, anyhow};
use base64::{Engine as _, engine::general_purpose};
use rand::Rng;
use tokio::net::UdpSocket;
use tokio::time::sleep;
use tracing::{error, info, instrument};
use rtp::header::Header;
use rtp::packet::Packet;
use webrtc_util::marshal::Marshal;
use rubato::{Resampler, SincFixedIn, SincInterpolationParameters, WindowFunction, SincInterpolationType};

use crate::audio::{self, AudioCache, linear_to_ulaw};
use crate::config::AppConfig;

const TARGET_SAMPLE_RATE: usize = 8000;

#[instrument(skip_all, fields(remote = %target_addr, uri))]
pub async fn send_announcement_from_uri(
    sock: Arc<UdpSocket>, 
    target_addr: SocketAddr, 
    audio_uri: String, 
    cache: AudioCache, 
    config: Arc<AppConfig>
) {
    // Log kirliliğini önlemek için URI'nin sadece şemasını ve başlangıcını logla
    let uri_preview = audio_uri.chars().take(70).collect::<String>();
    tracing::Span::current().record("uri", &uri_preview.as_str());
    info!("Anons gönderimi başlıyor...");
    
    let result = if audio_uri.starts_with("file:///") {
        let file_path = audio_uri.strip_prefix("file:///").unwrap_or(&audio_uri);
        load_from_file_and_send(&sock, target_addr, file_path, &cache, &config).await
    } else if audio_uri.starts_with("data:audio/wav;base64,") {
        let base64_data = audio_uri.strip_prefix("data:audio/wav;base64,").unwrap();
        load_from_base64_and_send(&sock, target_addr, base64_data).await
    } else {
        Err(anyhow!("Desteklenmeyen URI şeması: {}", uri_preview))
    };

    if let Err(e) = result {
        error!(error = ?e, "Anons gönderimi sırasında bir hata oluştu.");
    }
}

async fn load_from_base64_and_send(
    sock: &Arc<UdpSocket>, 
    target_addr: SocketAddr, 
    base64_data: &str
) -> Result<()> {
    info!("Base64 verisinden ses yükleniyor...");
    let wav_bytes = general_purpose::STANDARD.decode(base64_data).context("Base64 verisi çözümlenemedi")?;
    
    let mut reader = hound::WavReader::new(std::io::Cursor::new(wav_bytes)).context("WAV verisi ayrıştırılamadı")?;
    let spec = reader.spec();
    let source_sample_rate = spec.sample_rate as usize;
    
    let samples_f32: Vec<f32> = reader.samples::<i16>().collect::<Result<Vec<_>, _>>()?.into_iter().map(|s| s as f32 / 32768.0).collect();

    let resampled_samples: Vec<i16> = if source_sample_rate != TARGET_SAMPLE_RATE {
        info!(from = source_sample_rate, to = TARGET_SAMPLE_RATE, "Ses yeniden örnekleniyor...");
        let params = SincInterpolationParameters {
            sinc_len: 256,
            f_cutoff: 0.95,
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: 256,
            window: WindowFunction::BlackmanHarris2,
        };
        let mut resampler = SincFixedIn::<f32>::new(
            TARGET_SAMPLE_RATE as f64 / source_sample_rate as f64,
            2.0,
            params,
            samples_f32.len(),
            1,
        )?;
        let resampled_f32 = resampler.process(&[samples_f32], None)?.remove(0);
        resampled_f32.into_iter().map(|s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16).collect::<Vec<i16>>()
    } else {
        samples_f32.into_iter().map(|s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16).collect::<Vec<i16>>()
    };

    send_rtp_stream(sock, target_addr, &resampled_samples).await
}

async fn load_from_file_and_send(
    sock: &Arc<UdpSocket>, 
    target_addr: SocketAddr, 
    audio_id: &str, 
    cache: &AudioCache, 
    config: &AppConfig
) -> Result<()> {
    let samples = audio::load_or_get_from_cache(cache, audio_id, &config.assets_base_path).await?;
    send_rtp_stream(sock, target_addr, &samples).await
}

async fn send_rtp_stream(
    sock: &Arc<UdpSocket>, 
    target_addr: SocketAddr, 
    samples: &[i16]
) -> Result<()> {
    let ssrc: u32 = rand::thread_rng().gen();
    let mut sequence_number: u16 = rand::thread_rng().gen();
    let mut timestamp: u32 = rand::thread_rng().gen();
    const SAMPLES_PER_PACKET: usize = 160;

    for chunk in samples.chunks(SAMPLES_PER_PACKET) {
        let packet = Packet {
            header: Header { version: 2, payload_type: 0, sequence_number, timestamp, ssrc, ..Default::default() },
            payload: chunk.iter().map(|&s| linear_to_ulaw(s)).collect(),
        };
        sock.send_to(&packet.marshal()?, target_addr).await?;
        sequence_number = sequence_number.wrapping_add(1);
        timestamp = timestamp.wrapping_add(SAMPLES_PER_PACKET as u32);
        sleep(Duration::from_millis(20)).await;
    }
    info!("Anons gönderimi tamamlandı.");
    Ok(())
}