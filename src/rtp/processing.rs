// sentiric-media-service/src/rtp/processing.rs

use crate::rtp::codecs;
use sentiric_rtp_core::{CodecFactory, CodecType, Encoder, AudioResampler};
use tracing::{info, warn, error};

/// AudioProcessor: AI Pipeline (16kHz) ile RTP (8kHz) arasındaki köprüdür.
pub struct AudioProcessor {
    encoder: Box<dyn Encoder>,
    accumulator: Vec<i16>, 
    current_codec: CodecType,
    // YENİ: Stateful Resampler (16k -> 8k)
    resampler: AudioResampler,
}

impl AudioProcessor {
    pub fn new(initial_codec: CodecType) -> Self {
        info!("🎛️ Audio Processor Initialized for Codec: {:?}", initial_codec);
        Self {
            encoder: CodecFactory::create_encoder(initial_codec),
            accumulator: Vec::with_capacity(8192), 
            current_codec: initial_codec,
            // 16k input, 8k output, chunk size dinamik olabilir ama başlangıç için 320 veriyoruz
            resampler: AudioResampler::new(16000, 8000, 320),
        }
    }

    pub fn update_codec(&mut self, new_codec: CodecType) {
        if self.current_codec != new_codec {
            info!("🔄 Switching Processor Codec: {:?} -> {:?}", self.current_codec, new_codec);
            self.current_codec = new_codec;
            self.encoder = CodecFactory::create_encoder(new_codec);
        }
    }

    pub fn push_data(&mut self, data: Vec<u8>) {
        if data.len() % 2 != 0 {
            warn!("⚠️ Malformed audio chunk received (odd length)");
            return;
        }

        let samples: Vec<i16> = data.chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        
        self.accumulator.extend(samples);
    }

    pub async fn process_frame(&mut self) -> Option<Vec<Vec<u8>>> {
        // AI'dan gelen 16kHz veriyi 20ms'lik bloklar halinde işle
        // 20ms @ 16kHz = 320 samples
        const FRAME_SIZE_16K: usize = 320; 
        
        if self.accumulator.len() < FRAME_SIZE_16K {
            return None;
        }

        let frame_16k: Vec<i16> = self.accumulator.drain(0..FRAME_SIZE_16K).collect();
        
        // 1. Resampling (16k -> 8k) - Stateful
        let frame_8k = self.resampler.process(&frame_16k).await;
        
        // 2. Encoding (8k -> RTP Payload)
        // Blocking çağrıya gerek yok, encode işlemi çok hafif
        let encoded = self.encoder.encode(&frame_8k);

        // RTP paket boyutlarına böl (G.711: 160 byte, G.729: 10 byte)
        let payload_size = if self.current_codec == CodecType::G729 { 10 } else { 160 };
        
        if encoded.is_empty() {
            return None;
        }

        Some(encoded.chunks(payload_size).map(|c| c.to_vec()).collect())
    }

    pub fn get_current_codec(&self) -> CodecType {
        self.current_codec
    }

    pub fn generate_silence(&mut self) -> Vec<u8> {
        self.encoder.encode(&vec![0i16; 160])
    }
}