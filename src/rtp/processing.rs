// sentiric-media-service/src/rtp/processing.rs

use crate::rtp::codecs;
use sentiric_rtp_core::{CodecFactory, CodecType, Encoder};
use tracing::{info, warn, error};

/// AudioProcessor: AI Pipeline (16kHz) ile RTP (8kHz) arasındaki köprüdür.
/// Resampling ve Buffering işlemlerini yönetir.
pub struct AudioProcessor {
    encoder: Box<dyn Encoder>,
    accumulator: Vec<i16>, 
    current_codec: CodecType,
}

impl AudioProcessor {
    pub fn new(initial_codec: CodecType) -> Self {
        info!("🎛️ Audio Processor Initialized for Codec: {:?}", initial_codec);
        Self {
            encoder: CodecFactory::create_encoder(initial_codec),
            accumulator: Vec::with_capacity(8192), // 500ms+ buffer safety
            current_codec: initial_codec,
        }
    }

    /// update_codec: Çağrı sırasında kodek değişirse (re-invite) işlemciyi günceller.
    pub fn update_codec(&mut self, new_codec: CodecType) {
        if self.current_codec != new_codec {
            info!("🔄 Switching Processor Codec: {:?} -> {:?}", self.current_codec, new_codec);
            self.current_codec = new_codec;
            self.encoder = CodecFactory::create_encoder(new_codec);
        }
    }

    /// push_data: AI Pipeline'dan (TTS) gelen 16kHz LPCM verisini biriktirir.
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

    /// process_frame: Biriktirilen veriyi 20ms'lik paketlere (frames) böler ve kodlar.
    pub async fn process_frame(&mut self) -> Option<Vec<Vec<u8>>> {
        // 16kHz'de 20ms = 320 örnek (samples)
        const FRAME_SIZE_16K: usize = 320; 
        
        if self.accumulator.len() < FRAME_SIZE_16K {
            return None;
        }

        // 20ms'lik dilimi al
        let frame_in: Vec<i16> = self.accumulator.drain(0..FRAME_SIZE_16K).collect();
        let encoder_type = self.current_codec;
        
        // Ağır kodlama işlemini CPU thread pool'a gönder
        let encoded_res = tokio::task::spawn_blocking(move || {
             codecs::encode_lpcm16_to_rtp(
                 &frame_in, 
                 codecs::AudioCodec::from_rtp_payload_type(encoder_type as u8).unwrap_or(codecs::AudioCodec::Pcmu)
             )
        }).await;

        match encoded_res {
            Ok(Ok(encoded)) => {
                // Kodlanmış veriyi RTP paket boyutlarına (örn: G.711 için 160 byte) böl
                let payload_size = if self.current_codec == CodecType::G729 { 10 } else { 160 };
                Some(encoded.chunks(payload_size).map(|c| c.to_vec()).collect())
            },
            Ok(Err(e)) => {
                error!("❌ Encoding error in processor: {}", e);
                None
            },
            Err(e) => {
                error!("❌ Processor thread panic: {}", e);
                None
            }
        }
    }

    pub fn get_current_codec(&self) -> CodecType {
        self.current_codec
    }

    /// generate_silence: Konuşma olmadığında jitter buffer'ı beslemek için sessizlik üretir.
    pub fn generate_silence(&mut self) -> Vec<u8> {
        // G.711 için sessizlik değeri (0x80 veya 0xFF) encoder tarafından üretilir.
        self.encoder.encode(&vec![0i16; 160])
    }
}