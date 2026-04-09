// Dosya: src/rtp/processing.rs
use sentiric_rtp_core::{AudioResampler, CodecFactory, CodecType, Encoder};
use tracing::{info, warn};

pub struct AudioProcessor {
    encoder: Box<dyn Encoder>,
    accumulator: Vec<i16>,
    current_codec: CodecType,
    resampler: AudioResampler,
}

impl AudioProcessor {
    pub fn new(initial_codec: CodecType) -> Self {
        info!(
            "🎛️ Audio Processor Initialized for Codec: {:?}",
            initial_codec
        );
        Self {
            encoder: CodecFactory::create_encoder(initial_codec),
            accumulator: Vec::with_capacity(8192),
            current_codec: initial_codec,
            resampler: AudioResampler::new(16000, 8000, 320),
        }
    }

    pub fn update_codec(&mut self, new_codec: CodecType) {
        if self.current_codec != new_codec {
            info!(
                "🔄 Switching Processor Codec: {:?} -> {:?}",
                self.current_codec, new_codec
            );
            self.current_codec = new_codec;
            self.encoder = CodecFactory::create_encoder(new_codec);
        }
    }

    pub fn push_data(&mut self, data: Vec<u8>) {
        // [CLIPPY FIX]: manual_is_multiple_of
        if !data.len().is_multiple_of(2) {
            warn!("⚠️ Malformed audio chunk received (odd length)");
            return;
        }

        let samples: Vec<i16> = data
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();

        self.accumulator.extend(samples);
    }

    pub async fn process_frame(&mut self) -> Option<Vec<Vec<u8>>> {
        const FRAME_SIZE_16K: usize = 320;

        if self.accumulator.len() < FRAME_SIZE_16K {
            return None;
        }

        let frame_16k: Vec<i16> = self.accumulator.drain(0..FRAME_SIZE_16K).collect();
        let frame_8k = self.resampler.process(&frame_16k);
        let encoded = self.encoder.encode(&frame_8k);

        let payload_size = if self.current_codec == CodecType::G729 {
            10
        } else {
            160
        };

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
