// sentiric-media-service/src/rtp/processing.rs

use crate::rtp::codecs;
use sentiric_rtp_core::{CodecFactory, CodecType, Encoder};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::info;

/// RTP oturumu içindeki ses işleme durumunu yöneten yapı.
/// Resampling, Encoding ve Buffering işlemlerini kapsüller.
pub struct AudioProcessor {
    // Giden ses (TTS -> RTP) için Encoder
    encoder: Box<dyn Encoder>,
    // Giden ses (TTS -> RTP) için Resampler (24k -> 8k)
    resampler: Arc<Mutex<codecs::StatefulResampler>>,
    // Encode edilmeyi bekleyen ham ses parçaları
    accumulator: Vec<f32>,
    // Şu an aktif olan Codec
    current_codec: CodecType,
    // Resampler'ın beklediği frame boyutu (Cache)
    resampler_frame_size: usize,
}

impl AudioProcessor {
    pub fn new(initial_codec: CodecType) -> Self {
        let resampler = codecs::StatefulResampler::new(24000, 8000).expect("Resampler başlatılamadı");
        let frame_size = resampler.input_frame_size;
        
        info!("🎛️ Audio Processor Başlatıldı. Initial Codec: {:?}", initial_codec);

        Self {
            encoder: CodecFactory::create_encoder(initial_codec),
            resampler: Arc::new(Mutex::new(resampler)),
            accumulator: Vec::with_capacity(4096),
            current_codec: initial_codec,
            resampler_frame_size: frame_size,
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

    /// Gelen ham ses verisini (bytes) biriktirir.
    pub fn push_data(&mut self, data: Vec<u8>) {
        let samples_f32: Vec<f32> = data.chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
            .collect();
        self.accumulator.extend(samples_f32);
    }

    /// Birikmiş veriyi işleyip RTP payload'larına dönüştürür.
    /// Eğer yeterli veri yoksa None döner.
    pub async fn process_frame(&mut self) -> Option<Vec<Vec<u8>>> {
        if self.accumulator.len() < self.resampler_frame_size {
            return None;
        }

        let frame_in: Vec<f32> = self.accumulator.drain(0..self.resampler_frame_size).collect();
        
        let resampler_clone = self.resampler.clone();
        
        let samples_8k_f32 = tokio::task::spawn_blocking(move || {
            let mut guard = resampler_clone.blocking_lock();
            guard.process(&frame_in)
        }).await.ok().and_then(|r| r.ok())?;

        let samples_8k_i16: Vec<i16> = samples_8k_f32.into_iter()
            .map(|s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16)
            .collect();
        
        let encoded = self.encoder.encode(&samples_8k_i16);
        
        let payload_size = if self.current_codec == CodecType::G729 { 20 } else { 160 };
        
        let mut packets = Vec::new();
        for chunk in encoded.chunks(payload_size) {
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