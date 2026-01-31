// sentiric-media-service/src/rtp/codecs.rs

use anyhow::{anyhow, Result};
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
// DÜZELTME: Sadece kullanılanları import ediyoruz (Decoder ve G711 uyarıları için silindi)
use sentiric_rtp_core::{CodecFactory, CodecType}; 

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCodec {
    Pcmu,
    Pcma,
    G729,
}

impl AudioCodec {
    pub fn from_rtp_payload_type(payload_type: u8) -> Result<Self> {
        match payload_type {
            0 => Ok(AudioCodec::Pcmu),
            8 => Ok(AudioCodec::Pcma),
            18 => Ok(AudioCodec::G729),
            _ => Err(anyhow!("Desteklenmeyen RTP payload tipi: {}", payload_type)),
        }
    }

    // RTP-CORE tipine dönüştürücü
    pub fn to_core_type(&self) -> CodecType {
        match self {
            AudioCodec::Pcmu => CodecType::PCMU,
            AudioCodec::Pcma => CodecType::PCMA,
            AudioCodec::G729 => CodecType::G729,
        }
    }
}

pub struct StatefulResampler {
    resampler: SincFixedIn<f32>,
    pub input_frame_size: usize,
}

impl StatefulResampler {
    pub fn new(source_rate: u32, target_rate: u32) -> Result<Self> {
        let params = SincInterpolationParameters {
            sinc_len: 256,
            f_cutoff: 0.95,
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: 256,
            window: WindowFunction::BlackmanHarris2,
        };

        let input_frame_size = (source_rate as f32 * 0.02) as usize;

        let resampler = SincFixedIn::<f32>::new(
            target_rate as f64 / source_rate as f64, 
            2.0, 
            params,
            input_frame_size, 
            1, 
        )?;

        Ok(Self { 
            resampler,
            input_frame_size,
        })
    }

    pub fn process(&mut self, samples_in: &[f32]) -> Result<Vec<f32>> {
        if samples_in.len() != self.input_frame_size {
            return Err(anyhow!(
                "Resampler size mismatch! Expected: {}, Got: {}",
                self.input_frame_size,
                samples_in.len()
            ));
        }

        let waves_in = vec![samples_in.to_vec()];
        let mut waves_out = self.resampler.process(&waves_in, None)?;
        
        if waves_out.is_empty() {
            return Err(anyhow!("Resampler produced no output"));
        }

        Ok(waves_out.remove(0))
    }
}

// --- MERKEZİ DECODE MANTIĞI ---

/// Herhangi bir desteklenen RTP paketini alır ve STT/Kayıt için 16kHz LPCM'e çevirir.
pub fn decode_rtp_to_lpcm16(
    payload: &[u8],
    codec: AudioCodec,
) -> Result<Vec<i16>> {
    // 1. RTP-CORE üzerinden ilgili çözücüyü (Decoder) oluştur
    let mut decoder = CodecFactory::create_decoder(codec.to_core_type());
    
    // 2. Çöz (Her zaman 8kHz döner - G.729 ve G.711 için)
    let samples_8k = decoder.decode(payload);
    
    // 3. 8kHz -> 16kHz Upsampling (Simple & Fast)
    Ok(upsample_8k_to_16k(samples_8k))
}

/// Giden ses için: 16k LPCM'den hedef RTP kodeğine çevirir.
pub fn encode_lpcm16_to_rtp(samples_16k: &[i16], target_codec: AudioCodec) -> Result<Vec<u8>> {
    // 16k -> 8k Downsampling
    let samples_8k_i16: Vec<i16> = samples_16k.iter().step_by(2).cloned().collect();

    // RTP-CORE üzerinden encoder oluştur ve encode et
    let mut encoder = CodecFactory::create_encoder(target_codec.to_core_type());
    Ok(encoder.encode(&samples_8k_i16))
}

fn upsample_8k_to_16k(samples_8k: Vec<i16>) -> Vec<i16> {
    let mut samples_16k = Vec::with_capacity(samples_8k.len() * 2);
    for s in samples_8k {
        samples_16k.push(s);
        samples_16k.push(s);
    }
    samples_16k
}