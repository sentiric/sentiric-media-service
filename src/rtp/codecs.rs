// sentiric-media-service/src/rtp/codecs.rs

use anyhow::{anyhow, Result};
use sentiric_rtp_core::{CodecFactory, CodecType}; // CodecFactory import'u kaldırıldı

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCodec {

    Pcmu,
    Pcma,
    G729,
    G722,
}

impl AudioCodec {
    pub fn from_rtp_payload_type(payload_type: u8) -> Result<Self> {
        match payload_type {
            0 => Ok(AudioCodec::Pcmu),
            8 => Ok(AudioCodec::Pcma),
            9 => Ok(AudioCodec::G722),
            18 => Ok(AudioCodec::G729),
            _ => Err(anyhow!("Desteklenmeyen RTP payload tipi: {}", payload_type)),
        }
    }

    pub fn to_core_type(&self) -> CodecType {
        match self {
            AudioCodec::Pcmu => CodecType::PCMU,
            AudioCodec::Pcma => CodecType::PCMA,
            AudioCodec::G729 => CodecType::G729,
            AudioCodec::G722 => CodecType::G722,
        }
    }
    
    pub fn to_payload_type(&self) -> u8 {
        match self {
            AudioCodec::Pcmu => 0,
            AudioCodec::Pcma => 8,
            AudioCodec::G722 => 9,
            AudioCodec::G729 => 18,
        }
    }
}


// --- MERKEZİ DECODE MANTIĞI (pub eklendi) ---

pub fn decode_rtp_to_lpcm16(
    payload: &[u8],
    codec: AudioCodec,
) -> Result<Vec<i16>> {
    let mut decoder = CodecFactory::create_decoder(codec.to_core_type());
    
    let samples = decoder.decode(payload);

    if codec.to_core_type().sample_rate() == 8000 {
        Ok(upsample_8k_to_16k(samples))
    } else {
        Ok(samples)
    }
}

pub fn encode_lpcm16_to_rtp(samples_16k: &[i16], target_codec: AudioCodec) -> Result<Vec<u8>> {
    let samples_to_encode = if target_codec.to_core_type().sample_rate() == 8000 {
        samples_16k.iter().step_by(2).cloned().collect()
    } else {
        samples_16k.to_vec()
    };

    let mut encoder = CodecFactory::create_encoder(target_codec.to_core_type());
    Ok(encoder.encode(&samples_to_encode))
}

fn upsample_8k_to_16k(samples_8k: Vec<i16>) -> Vec<i16> {
    let mut samples_16k = Vec::with_capacity(samples_8k.len() * 2);
    for s in samples_8k {
        samples_16k.push(s); 
        samples_16k.push(s);
    }
    samples_16k
}