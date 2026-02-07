// sentiric-media-service/src/rtp/processing.rs

use crate::rtp::codecs;
use sentiric_rtp_core::{CodecFactory, CodecType, Encoder};
use tracing::info;

pub struct AudioProcessor {
    encoder: Box<dyn Encoder>,
    accumulator: Vec<i16>, 
    current_codec: CodecType,
}

impl AudioProcessor {
    pub fn new(initial_codec: CodecType) -> Self {
        info!("🎛️ Audio Processor Init: {:?}", initial_codec);
        Self {
            encoder: CodecFactory::create_encoder(initial_codec),
            accumulator: Vec::with_capacity(4096),
            current_codec: initial_codec,
        }
    }

    pub fn update_codec(&mut self, new_codec: CodecType) {
        if self.current_codec != new_codec {
            self.current_codec = new_codec;
            self.encoder = CodecFactory::create_encoder(new_codec);
        }
    }

    pub fn push_data(&mut self, data: Vec<u8>) {
        let samples: Vec<i16> = data.chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        self.accumulator.extend(samples);
    }

    pub async fn process_frame(&mut self) -> Option<Vec<Vec<u8>>> {
        const FRAME_SIZE_16K: usize = 320; // 20ms
        if self.accumulator.len() < FRAME_SIZE_16K { return None; }

        let frame_in: Vec<i16> = self.accumulator.drain(0..FRAME_SIZE_16K).collect();
        let encoder_type = self.current_codec;
        
        let encoded = tokio::task::spawn_blocking(move || {
             codecs::encode_lpcm16_to_rtp(&frame_in, codecs::AudioCodec::from_rtp_payload_type(encoder_type as u8).unwrap())
        }).await.ok().and_then(|r| r.ok())?;

        let payload_size = if self.current_codec == CodecType::G729 { 10 } else { 160 };
        Some(encoded.chunks(payload_size).map(|c| c.to_vec()).collect())
    }

    pub fn get_current_codec(&self) -> CodecType {
        self.current_codec
    }

    pub fn generate_silence(&mut self) -> Vec<u8> {
        self.encoder.encode(&vec![0i16; 160])
    }
}