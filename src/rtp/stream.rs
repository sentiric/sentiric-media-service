use std::io::Cursor;
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
use rubato::{Resampler, SincFixedIn, SincInterpolationType, SincInterpolationParameters, WindowFunction};

// --- EKSİK İMPORTLAR BURAYA EKLENDİ ---
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
// --- ---

use crate::audio::{self, AudioCache, linear_to_ulaw};
use crate::config::AppConfig;

const TARGET_SAMPLE_RATE: u32 = 8000;

#[instrument(skip_all, fields(remote = %target_addr, uri))]
pub async fn send_announcement_from_uri(
    sock: Arc<UdpSocket>, 
    target_addr: SocketAddr, 
    audio_uri: String, 
    cache: AudioCache, 
    config: Arc<AppConfig>
) {
    let uri_preview = audio_uri.chars().take(70).collect::<String>();
    tracing::Span::current().record("uri", &uri_preview.as_str());
    info!("Anons gönderimi başlıyor...");
    
    let result = if audio_uri.starts_with("file:///") {
        let file_path = audio_uri.strip_prefix("file:///").unwrap_or(&audio_uri);
        load_from_file_and_send(&sock, target_addr, file_path, &cache, &config).await
    } else if audio_uri.starts_with("data:") {
        load_from_data_uri_and_send(&sock, target_addr, &audio_uri).await
    } else {
        Err(anyhow!("Desteklenmeyen URI şeması: {}", uri_preview))
    };

    if let Err(e) = result {
        error!(error = ?e, "Anons gönderimi sırasında bir hata oluştu.");
    }
}

async fn load_from_data_uri_and_send(
    sock: &Arc<UdpSocket>, 
    target_addr: SocketAddr, 
    data_uri: &str
) -> Result<()> {
    info!("Data URI'sinden ses yükleniyor...");
    let (_media_type, base64_data) = data_uri
        .strip_prefix("data:")
        .and_then(|s| s.split_once(";base64,"))
        .context("Geçersiz data URI formatı")?;

    let audio_bytes = general_purpose::STANDARD.decode(base64_data).context("Base64 verisi çözümlenemedi")?;
    
    let samples = decode_audio_with_symphonia(audio_bytes)?;
    
    send_rtp_stream(sock, target_addr, &samples).await
}

fn decode_audio_with_symphonia(audio_bytes: Vec<u8>) -> Result<Vec<i16>> {
    let mss = MediaSourceStream::new(Box::new(Cursor::new(audio_bytes)), Default::default());
    let hint = Hint::new();
    let meta_opts: MetadataOptions = Default::default();
    let fmt_opts: FormatOptions = Default::default();
    let probed = symphonia::default::get_probe().format(&hint, mss, &fmt_opts, &meta_opts)?;

    let mut format = probed.format;
    let track = format.tracks().iter().find(|t| t.codec_params.codec != CODEC_TYPE_NULL).context("Uyumlu ses kanalı bulunamadı")?;

    let source_sample_rate = track.codec_params.sample_rate.context("Örnekleme oranı bulunamadı")?;
    let decoder_opts: DecoderOptions = Default::default();
    let mut decoder = symphonia::default::get_codecs().make(&track.codec_params, &decoder_opts)?;

    let mut all_samples_f32: Vec<f32> = Vec::new();
    loop {
        match format.next_packet() {
            Ok(packet) => {
                let decoded = decoder.decode(&packet)?;
                let mut sample_buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, *decoded.spec());
                sample_buf.copy_interleaved_ref(decoded);
                all_samples_f32.extend_from_slice(sample_buf.samples());
            }
            Err(SymphoniaError::IoError(ref err)) if err.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.into()),
        }
    }
    
    let resampled_samples: Vec<i16> = if source_sample_rate != TARGET_SAMPLE_RATE {
        info!(from = source_sample_rate, to = TARGET_SAMPLE_RATE, "Ses yeniden örnekleniyor...");
        let params = SincInterpolationParameters {
            sinc_len: 256, f_cutoff: 0.95, interpolation: SincInterpolationType::Linear,
            oversampling_factor: 256, window: WindowFunction::BlackmanHarris2,
        };
        let mut resampler = SincFixedIn::<f32>::new(
            TARGET_SAMPLE_RATE as f64 / source_sample_rate as f64,
            2.0,
            params,
            all_samples_f32.len(),
            1,
        )?;
        let resampled_f32 = resampler.process(&[all_samples_f32], None)?.remove(0);
        resampled_f32.into_iter().map(|s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16).collect::<Vec<i16>>()
    } else {
        all_samples_f32.into_iter().map(|s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16).collect::<Vec<i16>>()
    };

    Ok(resampled_samples)
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