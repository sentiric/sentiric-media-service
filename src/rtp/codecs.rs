// src/rtp/codecs.rs

use anyhow::{anyhow, Result};
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use sentiric_rtp_core::G711; // rtp-core G711 fonksiyonlarını kullanacağız

// --- CONSTANTS ---
// 20ms frame size @ 16kHz input -> 320 samples
pub const RESAMPLER_INPUT_FRAME_SIZE: usize = 320; 

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCodec {
    Pcmu,
    Pcma,
}

impl AudioCodec {
    pub fn from_rtp_payload_type(payload_type: u8) -> Result<Self> {
        match payload_type {
            0 => Ok(AudioCodec::Pcmu),
            8 => Ok(AudioCodec::Pcma),
            _ => Err(anyhow!("Desteklenmeyen RTP payload tipi: {}", payload_type)),
        }
    }
}

// --- StatefulResampler (TTS Akışı İçin) ---
// Rubato resampler'ını ve durumunu sarmalar.
pub struct StatefulResampler {
    resampler: SincFixedIn<f32>,
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

        let resampler = SincFixedIn::<f32>::new(
            target_rate as f64 / source_rate as f64, 
            2.0, 
            params,
            RESAMPLER_INPUT_FRAME_SIZE, 
            1, 
        )?;

        Ok(Self { resampler })
    }

    pub fn process(&mut self, samples_in: &[f32]) -> Result<Vec<f32>> {
        if samples_in.len() != RESAMPLER_INPUT_FRAME_SIZE {
            return Err(anyhow!(
                "Resampler input size mismatch! Expected: {}, Got: {}",
                RESAMPLER_INPUT_FRAME_SIZE,
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

// --- Helper Functions (Dosya Oynatma İçin) ---

/// Gelen 16kHz LPCM verisini 8kHz'e düşürüp G.711 (PCMA/PCMU) olarak kodlar.
/// Not: Bu fonksiyon `stream.rs` tarafından kullanılır ve basit bir decimation (atlama) yapar.
/// TTS için yukarıdaki StatefulResampler kullanılır.
pub fn encode_lpcm16_to_g711(samples_16k: &[i16], target_codec: AudioCodec) -> Result<Vec<u8>> {
    // 1. Basit Downsampling (16k -> 8k)
    // Her 2 örnekten 1'ini alıyoruz. Dosya okumalarında bu yeterli performansı verir.
    let samples_8k_i16: Vec<i16> = samples_16k.iter().step_by(2).cloned().collect();

    // 2. Encode (G.711)
    let g711_payload: Vec<u8> = match target_codec {
        AudioCodec::Pcmu => samples_8k_i16.iter().map(|&s| G711::linear_to_ulaw(s)).collect(),
        AudioCodec::Pcma => samples_8k_i16.iter().map(|&s| G711::linear_to_alaw(s)).collect(),
    };

    Ok(g711_payload)
}

/// Gelen g711 payload'unu LPCM16'ya çevirir (Gelen ses için)
pub fn decode_g711_to_lpcm16(
    payload: &[u8],
    codec: AudioCodec,
    _resampler: &mut StatefulResampler, // Gelecekte upsampling için rezerve
) -> Result<Vec<i16>> {
    // Basit Upsampling (8k -> 16k)
    // Her örneği iki kere yazıyoruz (Linear Interpolation yerine Nearest Neighbor)
    let mut samples_16k = Vec::with_capacity(payload.len() * 2);
    
    for &byte in payload {
        let pcm_val = match codec {
            // G711 modülündeki decode fonksiyonları olmadığı için (rtp-core'da private olabilir)
            // burada manuel decode yerine, rtp-core'un decoder'ını kullanmak isterdik ama 
            // linear_to_ulaw encode yönlüdür.
            // Bu proje kapsamında decode çok kritik değil (STT için), o yüzden basit bırakıyoruz.
            // STT için zaten G.711 -> PCM dönüşümü gerekiyor.
            
            // Eğer rtp-core'da decode yoksa basit bir lookup tablosu veya formül gerekir.
            // Şimdilik 0 dönüyorum çünkü inbound akış bu issue'nun konusu değil.
            // STT çalışması için buranın dolu olması gerekirse rtp-core güncellenmeli.
            AudioCodec::Pcmu => ulaw_to_linear(byte),
            AudioCodec::Pcma => alaw_to_linear(byte),
        };
        
        samples_16k.push(pcm_val);
        samples_16k.push(pcm_val);
    }

    Ok(samples_16k)
}

// Basit G.711 Decoder Yardımcıları
fn ulaw_to_linear(u_val: u8) -> i16 {
    let t = !u_val;
    let mut t16 = ((t & 0xf) as i16) << 3;
    t16 += 0x84;
    t16 <<= (t & 0x70) >> 4;
    t16 -= 0x84;
    if (t & 0x80) == 0 { -t16 } else { t16 }
}

fn alaw_to_linear(a_val: u8) -> i16 {
    let t = a_val ^ 0x55;
    let mut t16 = ((t & 0xf) as i16) << 4;
    t16 += 8;
    if (t & 0x70) != 0 {
        t16 += 0x100;
        let shift = ((t & 0x70) >> 4) - 1;
        t16 <<= shift;
    }
    if (t & 0x80) == 0 { -t16 } else { t16 }
}