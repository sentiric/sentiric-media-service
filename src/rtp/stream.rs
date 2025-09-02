// File: src/rtp/stream.rs (TAM, EKSİKSİZ VE KESİNLİKLE DERLENEBİLİR NİHAİ HALİ)
use crate::rtp::codecs::{self, AudioCodec};
use anyhow::{Context, Result};
use rand::Rng;
use rtp::header::Header;
use rtp::packet::Packet;
use std::io::Cursor;
use std::net::SocketAddr;
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
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use webrtc_util::marshal::Marshal;
use rubato::Resampler;

pub const INTERNAL_SAMPLE_RATE: u32 = 16000;

pub async fn send_rtp_stream(
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

    let mut ticker = tokio::time::interval(Duration::from_millis(20));

    for chunk in g711_payload.chunks(SAMPLES_PER_PACKET) {
        // === DEĞİŞİKLİK BURADA: `select!` bloğu döngünün içine alındı ===
        // Bu, her iterasyonda iptal kontrolü yapmamızı sağlar ve makronun yapısına uyar.
        tokio::select! {
            biased; // İptal kontrolünü öncelikli hale getirir
            _ = token.cancelled() => {
                info!("RTP akışı dışarıdan iptal edildi.");
                return Ok(()); // Future'dan erken çıkış
            }
            _ = ticker.tick() => {
                let packet = Packet {
                    header: Header {
                        version: 2, payload_type: rtp_payload_type, sequence_number,
                        timestamp, ssrc, ..Default::default()
                    },
                    payload: chunk.to_vec().into(),
                };
                if let Err(e) = sock.send_to(&packet.marshal()?, target_addr).await {
                    warn!(error = %e, "RTP paketi gönderilemedi.");
                    break; // Döngüden çık, fonksiyon sonlanacak
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