// sentiric-media-service/src/rtp/codecs.rs

use anyhow::{anyhow, Result};
use sentiric_rtp_core::{CodecFactory, CodecType, simple_resample};
use tracing::trace;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCodec {
    G729,
    Pcmu,
    Pcma,
    TelephoneEvent,
}

impl AudioCodec {
    pub fn from_rtp_payload_type(payload_type: u8) -> Result<Self> {
        match payload_type {
            18 => Ok(AudioCodec::G729),
            0 => Ok(AudioCodec::Pcmu),
            8 => Ok(AudioCodec::Pcma),
            101 => Ok(AudioCodec::TelephoneEvent),
            _ => Err(anyhow!("Unsupported RTP payload type: {}", payload_type)),
        }
    }

    pub fn to_core_type(&self) -> CodecType {
        match self {
            AudioCodec::G729 => CodecType::G729,
            AudioCodec::Pcmu => CodecType::PCMU,
            AudioCodec::Pcma => CodecType::PCMA,
            AudioCodec::TelephoneEvent => CodecType::TelephoneEvent,
        }
    }
    
    pub fn to_payload_type(&self) -> u8 {
        self.to_core_type() as u8
    }
}

pub fn decode_rtp_to_lpcm16(payload: &[u8], codec: AudioCodec) -> Result<Vec<i16>> {
    if codec == AudioCodec::TelephoneEvent {
        return Ok(vec![]); 
    }

    let mut decoder = CodecFactory::create_decoder(codec.to_core_type());
    let samples_8k = decoder.decode(payload);
    
    // 8k -> 16k Upsample
    Ok(simple_resample(&samples_8k, 8000, 16000))
}

pub fn encode_lpcm16_to_rtp(samples_16k: &[i16], target_codec: AudioCodec) -> Result<Vec<u8>> {
    if target_codec == AudioCodec::TelephoneEvent {
        return Ok(vec![]);
    }
    
    // 16k -> 8k Downsample
    let samples_8k = simple_resample(samples_16k, 16000, 8000);
    trace!("Downsampled {} samples to {} samples (16k->8k)", samples_16k.len(), samples_8k.len());
    
    let mut encoder = CodecFactory::create_encoder(target_codec.to_core_type());
    Ok(encoder.encode(&samples_8k))
}