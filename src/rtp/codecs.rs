// sentiric-media-service/src/rtp/codecs.rs

use anyhow::{anyhow, Result};
use sentiric_rtp_core::{CodecFactory, CodecType};
use tracing::trace;

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
            _ => Err(anyhow!("Unsupported RTP payload type: {}", payload_type)),
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

/// RTP -> LPCM (16kHz)
/// AI için gelen sesi 16kHz'e çıkarır (Upsampling).
pub fn decode_rtp_to_lpcm16(payload: &[u8], codec: AudioCodec) -> Result<Vec<i16>> {
    let mut decoder = CodecFactory::create_decoder(codec.to_core_type());
    let samples = decoder.decode(payload);
    
    // G.711 (8k) -> AI (16k)
    // Her örneği iki kere yazarak frekansı ikiye katlıyoruz.
    if codec.to_core_type().sample_rate() == 8000 {
        let mut samples_16k = Vec::with_capacity(samples.len() * 2);
        for s in &samples {
            samples_16k.push(*s); 
            samples_16k.push(*s);
        }
        // trace!("Upsampled {} samples to {} samples (8k->16k)", samples.len(), samples_16k.len());
        Ok(samples_16k)
    } else {
        Ok(samples)
    }
}

/// LPCM (16kHz) -> RTP
/// AI'dan gelen 16kHz sesi, telefon hattı için uygun hıza dönüştürür.
pub fn encode_lpcm16_to_rtp(samples_16k: &[i16], target_codec: AudioCodec) -> Result<Vec<u8>> {
    let target_rate = target_codec.to_core_type().sample_rate();
    
    // [CRITICAL FIX]: Deep Voice Sorunu Çözümü
    // Eğer hedef kodek 8000Hz ise (PCMA/PCMU/G729), ama elimizdeki veri 16000Hz ise,
    // veriyi yarıya indirmeliyiz (Downsampling / Decimation).
    // Aksi takdirde ses yarı hızda ve kalın çıkar.
    let samples_to_encode = if target_rate == 8000 {
        // !!! CRITICAL FIX: DEEP VOICE / SLOW MOTION AUDIO !!!
        // 16kHz veriyi 8kHz hatta basarsak ses yarı hızda (kalın) çıkar.
        // Çözüm: Downsampling (Decimation). Her 2 örnekten 1'ini al.
        // Örn: [A, B, C, D] -> [A, C]        
        let downsampled: Vec<i16> = samples_16k.iter().step_by(2).cloned().collect();
        trace!("Downsampled {} samples to {} samples (16k->8k)", samples_16k.len(), downsampled.len());
        downsampled
    } else {
        // G.722 gibi 16k destekleyen kodekler için olduğu gibi bırak
        samples_16k.to_vec()
    };
    
    let mut encoder = CodecFactory::create_encoder(target_codec.to_core_type());
    Ok(encoder.encode(&samples_to_encode))
}