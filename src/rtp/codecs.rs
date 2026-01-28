// sentiric-media-service/src/rtp/codecs.rs

use anyhow::{anyhow, Result};
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use sentiric_rtp_core::G711; 

// [FIX] 24kHz input için 20ms frame size (24000 * 0.02 = 480 samples)
// Coqui XTTS v2 native 24kHz stream eder.
pub const RESAMPLER_INPUT_FRAME_SIZE: usize = 480; 

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

        // Input frame size hesaplama (20ms için):
        // 24000 Hz -> 480 sample
        // 16000 Hz -> 320 sample
        // 8000 Hz  -> 160 sample
        let input_frame_size = (source_rate as f32 * 0.02) as usize;

        // Validasyon: Rubato sabit boyut ister
        if input_frame_size != RESAMPLER_INPUT_FRAME_SIZE && source_rate == 24000 {
             return Err(anyhow!("Internal Logic Error: 24k rate must match fixed buffer size 480"));
        }

        let resampler = SincFixedIn::<f32>::new(
            target_rate as f64 / source_rate as f64, 
            2.0, 
            params,
            input_frame_size, 
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

// 16k LPCM'den G.711'e (RTP için) - BU FONKSİYON HALA 16K KABUL EDİYOR OLABİLİR AMA KULLANMIYORUZ
// Bizim asıl akışımız session.rs içinde manuel yapılıyor.
pub fn encode_lpcm16_to_g711(samples_16k: &[i16], target_codec: AudioCodec) -> Result<Vec<u8>> {
    // 16k -> 8k Basit Decimation
    let samples_8k_i16: Vec<i16> = samples_16k.iter().step_by(2).cloned().collect();

    let g711_payload: Vec<u8> = match target_codec {
        AudioCodec::Pcmu => samples_8k_i16.iter().map(|&s| G711::linear_to_ulaw(s)).collect(),
        AudioCodec::Pcma => samples_8k_i16.iter().map(|&s| G711::linear_to_alaw(s)).collect(),
    };

    Ok(g711_payload)
}

// G.711'den 16k LPCM'e (STT için)
pub fn decode_g711_to_lpcm16(
    payload: &[u8],
    codec: AudioCodec,
    _resampler: &mut StatefulResampler,
) -> Result<Vec<i16>> {
    let samples_8k: Vec<i16> = match codec {
        AudioCodec::Pcmu => payload.iter().map(|&b| G711::ulaw_to_linear(b)).collect(),
        AudioCodec::Pcma => payload.iter().map(|&b| G711::alaw_to_linear(b)).collect(),
    };
    
    // 8k -> 16k Basit Upsampling
    let mut samples_16k = Vec::with_capacity(samples_8k.len() * 2);
    for s in samples_8k {
        samples_16k.push(s);
        samples_16k.push(s);
    }
    
    Ok(samples_16k)
}