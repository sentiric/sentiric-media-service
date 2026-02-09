// sentiric-media-service/src/rtp/codecs.rs

use anyhow::{anyhow, Result};
use sentiric_rtp_core::{CodecFactory, CodecType};
use tracing::trace;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCodec {
    G729,
    Pcmu,

    Pcma,
    // G722 removed for stability
}


// ============================================================================
// !!! PRODUCTION READY CODECS !!!
// 
// [STABLE] G.729: Düşük bant genişliği, yüksek kararlılık.
// [STABLE] PCMU: Yüksek kalite, kayıpsız.
// [STABLE] PCMA: Cızırtılı
// ============================================================================

// rtp core benzer işlemler yapıyoruz????
// Kendimizi tekrar mı ediyoruz?

impl AudioCodec {
    pub fn from_rtp_payload_type(payload_type: u8) -> Result<Self> {
        match payload_type {
            18 => Ok(AudioCodec::G729),
            0 => Ok(AudioCodec::Pcmu),

            8 => Ok(AudioCodec::Pcma),
            // 9 => Ok(AudioCodec::G722), // Removed

            _ => Err(anyhow!("Unsupported RTP payload type: {}", payload_type)),
        }
    }

    pub fn to_core_type(&self) -> CodecType {
        match self {
            AudioCodec::G729 => CodecType::G729,
            AudioCodec::Pcmu => CodecType::PCMU,
            //
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
    // Lineer İnterpolasyon ile Upsampling (Daha yumuşak geçiş için)
    if codec.to_core_type().sample_rate() == 8000 {
        let mut samples_16k = Vec::with_capacity(samples.len() * 2);
        for i in 0..samples.len() {
            let current = samples[i];
            samples_16k.push(current);
            
            // Bir sonraki örnekle şimdiki örneğin ortalamasını al (Smoothing)
            if i + 1 < samples.len() {
                let next = samples[i+1];
                let avg = ((current as i32 + next as i32) / 2) as i16;
                samples_16k.push(avg);
            } else {
                samples_16k.push(current);
            }
        }
        Ok(samples_16k)
    } else {
        Ok(samples)
    }
}

/// LPCM (16kHz) -> RTP
/// AI'dan gelen 16kHz sesi, telefon hattı için uygun hıza dönüştürür.
pub fn encode_lpcm16_to_rtp(samples_16k: &[i16], target_codec: AudioCodec) -> Result<Vec<u8>> {
    let target_rate = target_codec.to_core_type().sample_rate();
    
    let samples_to_encode = if target_rate == 8000 {
        // [CRITICAL AUDIO FIX]: Averaging Downsampling
        let mut downsampled = Vec::with_capacity(samples_16k.len() / 2);
        for chunk in samples_16k.chunks(2) {
            if chunk.len() == 2 {
                // (Sample1 + Sample2) / 2 -> Daha yumuşak ses
                let avg = ((chunk[0] as i32 + chunk[1] as i32) / 2) as i16;
                downsampled.push(avg);
            } else if chunk.len() == 1 {
                downsampled.push(chunk[0]);
            }
        }
        
        trace!("Downsampled (Avg) {} samples to {} samples (16k->8k)", samples_16k.len(), downsampled.len());
        downsampled
    } else {
        samples_16k.to_vec()
    };
    
    let mut encoder = CodecFactory::create_encoder(target_codec.to_core_type());
    Ok(encoder.encode(&samples_to_encode))
}