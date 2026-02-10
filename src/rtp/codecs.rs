// sentiric-media-service/src/rtp/codecs.rs

use anyhow::{anyhow, Result};
use sentiric_rtp_core::{CodecFactory, CodecType, Resampler}; // Resampler eklendi
use tracing::trace;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCodec {
    G729,
    Pcmu,
    Pcma,
}

impl AudioCodec {
    pub fn from_rtp_payload_type(payload_type: u8) -> Result<Self> {
        match payload_type {
            18 => Ok(AudioCodec::G729),
            0 => Ok(AudioCodec::Pcmu),
            8 => Ok(AudioCodec::Pcma),
            _ => Err(anyhow!("Unsupported RTP payload type: {}", payload_type)),
        }
    }

    pub fn to_core_type(&self) -> CodecType {
        match self {
            AudioCodec::G729 => CodecType::G729,
            AudioCodec::Pcmu => CodecType::PCMU,
            AudioCodec::Pcma => CodecType::PCMA,
        }
    }
    
    pub fn to_payload_type(&self) -> u8 {
        match self {
            AudioCodec::G729 => 18,
            AudioCodec::Pcmu => 0,
            AudioCodec::Pcma => 8,
        }
    }
}

/// RTP -> LPCM (16kHz)
/// AI için gelen sesi 16kHz'e çıkarır (Upsampling).
pub fn decode_rtp_to_lpcm16(payload: &[u8], codec: AudioCodec) -> Result<Vec<i16>> {
    let mut decoder = CodecFactory::create_decoder(codec.to_core_type());
    let samples = decoder.decode(payload);
    
    // G.711/G.729 (8k) -> AI (16k)
    // [ARCHITECTURAL UPDATE]: Manual loop removed. Using centralized DSP logic.
    if codec.to_core_type().sample_rate() == 8000 {
        Ok(Resampler::upsample_linear_8k_to_16k(&samples))
    } else {
        Ok(samples)
    }
}

/// LPCM (16kHz) -> RTP
/// AI'dan gelen 16kHz sesi, telefon hattı için uygun hıza dönüştürür.
pub fn encode_lpcm16_to_rtp(samples_16k: &[i16], target_codec: AudioCodec) -> Result<Vec<u8>> {
    let target_rate = target_codec.to_core_type().sample_rate();
    
    let samples_to_encode = if target_rate == 8000 {
        // [ARCHITECTURAL UPDATE]: Manual averaging loop removed. Using centralized DSP logic.
        let downsampled = Resampler::downsample_average_16k_to_8k(samples_16k);
        trace!("Downsampled (DSP) {} samples to {} samples (16k->8k)", samples_16k.len(), downsampled.len());
        downsampled
    } else {
        samples_16k.to_vec()
    };
    
    let mut encoder = CodecFactory::create_encoder(target_codec.to_core_type());
    Ok(encoder.encode(&samples_to_encode))
}