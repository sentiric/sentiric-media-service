// src/rtp/codecs.rs

use anyhow::{anyhow, Result};
use sentiric_rtp_core::{simple_resample, CodecFactory, CodecType};
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

// [KALDIRILDI]: decode_rtp_to_lpcm16 ve decode_rtp_native_8k fonksiyonları kaldırıldı.
// Nedeni: G.729 "Stateful" bir kodektir. Bu fonksiyonlar her çağrıldığında
// yeni bir decoder ürettiği için ses robotikleşiyor ve bozuluyordu.
// Decode işlemi artık doğrudan src/rtp/session.rs içinde stateful olarak yapılıyor.

// [KORUNDU]: Encode işlemi TTS'ten gelen saf (durumsuz) 16k sesi sıkıştırdığı için burada kalabilir.
pub fn encode_lpcm16_to_rtp(samples_16k: &[i16], target_codec: AudioCodec) -> Result<Vec<u8>> {
    if target_codec == AudioCodec::TelephoneEvent {
        return Ok(vec![]);
    }

    // 16k -> 8k Downsample (Telekom standardı)
    let samples_8k = simple_resample(samples_16k, 16000, 8000);
    trace!(
        "Downsampled {} samples to {} samples (16k->8k)",
        samples_16k.len(),
        samples_8k.len()
    );

    let mut encoder = CodecFactory::create_encoder(target_codec.to_core_type());
    Ok(encoder.encode(&samples_8k))
}
