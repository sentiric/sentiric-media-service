// File: src/rtp/stream.rs (TAM VE EKSİKSİZ NİHAİ HALİ)
use crate::config::AppConfig;
use crate::rtp::codecs::{self, AudioCodec};
use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose, Engine as _};
use rand::Rng;
use rtp::header::Header;
use rtp::packet::Packet;
use std::io::Cursor;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use tokio::net::UdpSocket;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, instrument, warn};
use webrtc_util::marshal::Marshal;
use crate::audio::AudioCache; // AudioCache için import
use rubato::Resampler; // === EKSİK OLAN VE ŞİMDİ EKLENEN KRİTİK IMPORT ===

pub const INTERNAL_SAMPLE_RATE: u32 = 16000;

#[instrument(skip_all, fields(remote = %target_addr, uri, codec = ?target_codec))]
pub async fn send_announcement_from_uri(
    sock: Arc<UdpSocket>,
    target_addr: SocketAddr,
    audio_uri: String,
    // === DEĞİŞİKLİK: Kullanılmayan 'cache' parametresi kaldırıldı ===
    // Bu fonksiyon artık doğrudan önbelleğe alma işlemi yapmıyor, bu sorumluluk session.rs'e taşındı.
    _cache: AudioCache,
    config: Arc<AppConfig>,
    token: CancellationToken,
    target_codec: AudioCodec,
) {
    let uri_preview = audio_uri.chars().take(70).collect::<String>();
    tracing::Span::current().record("uri", &uri_preview.as_str());
    info!("Anons gönderimi başlıyor...");

    let result: Result<()> = async {
        let samples_16khz = if let Some(path_part) = audio_uri.strip_prefix("file://") {
            let mut final_path = PathBuf::from(&config.assets_base_path);
            final_path.push(path_part.trim_start_matches('/'));
            // Doğrudan dosyayı okuyup işliyoruz. Önbelleğe alma işi `session.rs`'de yapılıyor.
            let audio_bytes = tokio::fs::read(&final_path).await?;
            Arc::new(decode_audio_with_symphonia(audio_bytes)?)
        } else if audio_uri.starts_with("data:") {
            info!("Data URI'sinden ses yükleniyor...");
            let (_media_type, base64_data) = audio_uri
                .strip_prefix("data:")
                .and_then(|s| s.split_once(";base64,"))
                .context("Geçersiz data URI formatı")?;
            let audio_bytes = general_purpose::STANDARD
                .decode(base64_data)
                .context("Base64 verisi çözümlenemedi")?;
            Arc::new(decode_audio_with_symphonia(audio_bytes)?)
        } else {
            return Err(anyhow!("Desteklenmeyen URI şeması: {}", uri_preview));
        };

        send_rtp_stream(&sock, target_addr, &samples_16khz, token, target_codec).await
    }
    .await;

    if let Err(e) = result {
        if e.downcast_ref::<tokio::time::error::Elapsed>().is_some()
            || e.to_string().contains("cancelled")
        {
            warn!(error = ?e, "Anons gönderimi iptal edildi veya zaman aşımına uğradı.");
        } else {
            error!(error = ?e, "Anons gönderimi sırasında bir hata oluştu.");
        }
    }
}

async fn send_rtp_stream(
    sock: &Arc<UdpSocket>,
    target_addr: SocketAddr,
    samples_16khz: &[i16],
    token: CancellationToken,
    target_codec: AudioCodec,
) -> Result<()> {
    let g711_payload = codecs::encode_lpcm16_to_g711(samples_16khz, target_codec)?;
    info!(
        source_samples = samples_16khz.len(),
        encoded_bytes = g711_payload.len(),
        "Ses, hedef kodeğe başarıyla encode edildi."
    );

    let ssrc: u32 = rand::thread_rng().gen();
    let mut sequence_number: u16 = rand::thread_rng().gen();
    let mut timestamp: u32 = rand::thread_rng().gen();
    const SAMPLES_PER_PACKET: usize = 160;

    let rtp_payload_type = match target_codec {
        AudioCodec::Pcmu => 0,
        AudioCodec::Pcma => 8,
    };

    for chunk in g711_payload.chunks(SAMPLES_PER_PACKET) {
        tokio::select! {
            biased;
            _ = token.cancelled() => { info!("RTP akışı dışarıdan iptal edildi."); return Ok(()); }
            _ = sleep(Duration::from_millis(20)) => {
                let packet = Packet {
                    header: Header {
                        version: 2, payload_type: rtp_payload_type, sequence_number,
                        timestamp, ssrc, ..Default::default()
                    },
                    payload: chunk.to_vec().into(),
                };
                if let Err(e) = sock.send_to(&packet.marshal()?, target_addr).await {
                    warn!(error = %e, "RTP paketi gönderilemedi.");
                    break;
                }
                sequence_number = sequence_number.wrapping_add(1);
                timestamp = timestamp.wrapping_add(SAMPLES_PER_PACKET as u32);
            }
        }
    }
    info!("Anons gönderimi tamamlandı.");
    token.cancel();
    Ok(())
}

pub fn decode_audio_with_symphonia(audio_bytes: Vec<u8>) -> Result<Vec<i16>> {
    let mss = MediaSourceStream::new(Box::new(Cursor::new(audio_bytes)), Default::default());
    let hint = Hint::new();
    let meta_opts: MetadataOptions = Default::default();
    let fmt_opts: FormatOptions = Default::default();
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &fmt_opts, &meta_opts)?;
    let mut format = probed.format;
    let track = format.tracks().iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .context("Uyumlu ses kanalı bulunamadı")?;
    let source_sample_rate = track.codec_params.sample_rate.context("Örnekleme oranı bulunamadı")?;
    let decoder_opts: DecoderOptions = Default::default();
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &decoder_opts)?;
    let mut all_samples_f32: Vec<f32> = Vec::new();
    loop {
        match format.next_packet() {
            Ok(packet) => {
                let decoded = decoder.decode(&packet)?;
                let mut sample_buf =
                    SampleBuffer::<f32>::new(decoded.capacity() as u64, *decoded.spec());
                sample_buf.copy_interleaved_ref(decoded);
                all_samples_f32.extend_from_slice(sample_buf.samples());
            }
            Err(SymphoniaError::IoError(ref err))
                if err.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break
            }
            Err(e) => return Err(e.into()),
        }
    }
    
    if source_sample_rate != INTERNAL_SAMPLE_RATE {
        info!(from = source_sample_rate, to = INTERNAL_SAMPLE_RATE, "Ses yeniden örnekleniyor...");
        let params = rubato::SincInterpolationParameters {
            sinc_len: 256, f_cutoff: 0.95,
            interpolation: rubato::SincInterpolationType::Linear,
            oversampling_factor: 256, window: rubato::WindowFunction::BlackmanHarris2,
        };
        let mut resampler = rubato::SincFixedIn::<f32>::new(
            INTERNAL_SAMPLE_RATE as f64 / source_sample_rate as f64,
            2.0, params, all_samples_f32.len(), 1,
        )?;
        let resampled_f32 = resampler.process(&[all_samples_f32], None)?.remove(0);
        Ok(resampled_f32.into_iter()
            .map(|s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16)
            .collect())
    } else {
        Ok(all_samples_f32.into_iter()
            .map(|s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16)
            .collect())
    }
}