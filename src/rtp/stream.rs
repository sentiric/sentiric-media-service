// sentiric-media-service/src/rtp/stream.rs

use crate::rtp::codecs::{self, AudioCodec};
use anyhow::{Result, anyhow}; 
use rand::Rng;
use sentiric_rtp_core::{RtpHeader, RtpPacket};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

// EKSİK OLAN KRİTİK IMPORT:
use rubato::{Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};

pub const INTERNAL_SAMPLE_RATE: u32 = 16000;

pub async fn send_rtp_stream(
    sock: &Arc<UdpSocket>,
    target_addr: SocketAddr,
    samples_16khz: &[i16],
    token: CancellationToken,
    target_codec: AudioCodec,
) -> Result<()> {
    // 1. Encode
    let encoded_payload = codecs::encode_lpcm16_to_g711(samples_16khz, target_codec)?;
    
    info!(
        source_samples = samples_16khz.len(),
        encoded_bytes = encoded_payload.len(),
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

    for chunk in encoded_payload.chunks(SAMPLES_PER_PACKET) {
        tokio::select! {
            biased;
            _ = token.cancelled() => {
                info!("RTP akışı dışarıdan iptal edildi.");
                return Ok(());
            }
            _ = ticker.tick() => {
                let header = RtpHeader::new(rtp_payload_type, sequence_number, timestamp, ssrc);
                let packet = RtpPacket {
                    header,
                    payload: chunk.to_vec(),
                };
                
                // .to_bytes() rtp-core'un sağladığı metottur
                if let Err(e) = sock.send_to(&packet.to_bytes(), target_addr).await {
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

// Basit ve Güvenli WAV Decoder
pub fn decode_audio_with_symphonia(audio_bytes: Vec<u8>) -> Result<Vec<i16>> {
    // WAV Header Kontrolü (RIFF....WAVE)
    if audio_bytes.len() < 44 || &audio_bytes[0..4] != b"RIFF" || &audio_bytes[8..12] != b"WAVE" {
         return Err(anyhow!("Geçersiz WAV formatı (Header eksik)"));
    }

    let sample_rate = u32::from_le_bytes([
        audio_bytes[24], audio_bytes[25], audio_bytes[26], audio_bytes[27]
    ]);
    
    let data_start = 44; 
    let raw_samples = &audio_bytes[data_start..];
    
    let samples_native: Vec<i16> = raw_samples
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();

    if sample_rate != INTERNAL_SAMPLE_RATE {
        info!(from = sample_rate, to = INTERNAL_SAMPLE_RATE, "Ses yeniden örnekleniyor (Basit Resampler)...");
        
        let pcm_f32: Vec<f32> = samples_native.iter().map(|s| *s as f32 / 32768.0).collect();
        
        let params = SincInterpolationParameters {
            sinc_len: 128, f_cutoff: 0.95,
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: 128, window: WindowFunction::BlackmanHarris2,
        };
        let mut resampler = SincFixedIn::<f32>::new(
            INTERNAL_SAMPLE_RATE as f64 / sample_rate as f64,
            2.0, params, pcm_f32.len(), 1,
        )?;
        
        let resampled_f32 = resampler.process(&[pcm_f32], None)?.remove(0);
        
        Ok(resampled_f32.into_iter()
            .map(|s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16)
            .collect())
            
    } else {
        Ok(samples_native)
    }
}