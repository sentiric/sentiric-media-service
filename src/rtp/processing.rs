// sentiric-media-service/src/rtp/processing.rs

use crate::rtp::codecs;
use sentiric_rtp_core::{CodecFactory, CodecType, Encoder};
// use std::sync::Arc; // Kaldırıldı
// use tokio::sync::Mutex; // Kaldırıldı
use tracing::info;

/// RTP oturumu içindeki ses işleme durumunu yöneten yapı.
pub struct AudioProcessor {
    encoder: Box<dyn Encoder>,
    accumulator: Vec<i16>,
    current_codec: CodecType,
}

impl AudioProcessor {
    pub fn new(initial_codec: CodecType) -> Self {
        info!("🎛️ Audio Processor Başlatıldı. Initial Codec: {:?}", initial_codec);

        Self {
            encoder: CodecFactory::create_encoder(initial_codec),
            accumulator: Vec::with_capacity(4096),
            current_codec: initial_codec,
        }
    }

    /// Codec değişikliği gerekirse encoder'ı yeniler.
    pub fn update_codec(&mut self, new_codec: CodecType) {
        if self.current_codec != new_codec {
            info!("🔄 Codec Switch: {:?} -> {:?}", self.current_codec, new_codec);
            self.current_codec = new_codec;
            self.encoder = CodecFactory::create_encoder(new_codec);
        }
    }

    /// Gelen ham 16kHz ses verisini (bytes) biriktirir ve i16'ya çevirir.
    pub fn push_data(&mut self, data: Vec<u8>) {
        let samples_i16: Vec<i16> = data.chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        self.accumulator.extend(samples_i16);
    }

    /// Birikmiş 16kHz veriyi işleyip RTP payload'larına dönüştürür.
    pub async fn process_frame(&mut self) -> Option<Vec<Vec<u8>>> {
        const SAMPLES_PER_FRAME_16K: usize = 320; 

        if self.accumulator.len() < SAMPLES_PER_FRAME_16K {
            return None;
        }

        let frame_in: Vec<i16> = self.accumulator.drain(0..SAMPLES_PER_FRAME_16K).collect();
        
        let encoder_type = self.current_codec;
        
        let encoded_payload = tokio::task::spawn_blocking(move || {
             // 16kHz PCM -> hedef kodeğe çevirme
             codecs::encode_lpcm16_to_rtp(&frame_in, codecs::AudioCodec::from_rtp_payload_type(encoder_type as u8).unwrap())
        }).await.ok().and_then(|r| r.ok())?;

        // G.729 paket boyutu 10 byte, G.711 paket boyutu 160 byte'dır.
        let payload_size = if self.current_codec == CodecType::G729 { 10 } else { 160 };
        
        let mut packets = Vec::new();
        for chunk in encoded_payload.chunks(payload_size) {
            packets.push(chunk.to_vec());
        }

        Some(packets)
    }

    /// Sessizlik (Silence) paketi üretir.
    pub fn generate_silence(&mut self) -> Vec<u8> {
        let silence_pcm = vec![0i16; 160];
        self.encoder.encode(&silence_pcm)
    }

    pub fn get_current_codec(&self) -> CodecType {
        self.current_codec
    }
    
    pub fn clear_buffer(&mut self) {
        self.accumulator.clear();
    }
}