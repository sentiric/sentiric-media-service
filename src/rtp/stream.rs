// sentiric-media-service/src/rtp/stream.rs

use crate::rtp::codecs::{self, AudioCodec};
use anyhow::Result;
// use rand::Rng; // Kaldırıldı
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn, debug};

// CORE ENTEGRASYONU
use sentiric_rtp_core::{Pacer, RtpHeader, RtpPacket};

pub async fn send_rtp_stream(
    sock: &Arc<UdpSocket>,
    target_addr: SocketAddr,
    samples_16khz: &[i16],
    token: CancellationToken,
    target_codec: AudioCodec,
) -> Result<()> {
    // 1. Encode
    let encoded_payload = codecs::encode_lpcm16_to_rtp(samples_16khz, target_codec)?;
    
    info!(
        source_samples = samples_16khz.len(),
        encoded_bytes = encoded_payload.len(),
        target = %target_addr,
        "Anons akışı başlatılıyor."
    );

    let is_valid_target = !target_addr.ip().is_unspecified() && target_addr.port() != 0;
    if !is_valid_target {
        warn!("⚠️ Hedef adres geçersiz. Paketler atlanacak.");
    }

    let ssrc: u32 = rand::Rng::gen(&mut rand::thread_rng());
    let mut sequence_number: u16 = rand::Rng::gen(&mut rand::thread_rng());
    let mut timestamp: u32 = rand::Rng::gen(&mut rand::thread_rng());
    
    // Payload tipini codec'ten al
    let rtp_payload_type = target_codec.to_payload_type();

    // Paket Boyutu ve Zaman Damgası Artışı
    let (packet_chunk_size, samples_per_packet) = match target_codec {
        AudioCodec::G729 => (10, 80),
        AudioCodec::G722 => (160, 320), // 16khz * 20ms = 320 samples
        _ => (160, 160), // G.711 için 8k * 20ms = 160 samples
    };


    // CORE: HİBRİT PACER BAŞLAT
    let mut pacer = Pacer::new(Duration::from_millis(20));

    for chunk in encoded_payload.chunks(packet_chunk_size) {
        if token.is_cancelled() {
            info!("RTP akışı iptal edildi.");
            return Ok(());
        }

        // KRİTİK: BEKLE VE HİZALA
        pacer.wait();

        if is_valid_target {
            let header = RtpHeader::new(rtp_payload_type, sequence_number, timestamp, ssrc);
            let packet = RtpPacket {
                header,
                payload: chunk.to_vec(),
            };
            
            if let Err(e) = sock.send_to(&packet.to_bytes(), target_addr).await {
                debug!(error = %e, "RTP paketi gönderilemedi.");
            }
        }
        
        sequence_number = sequence_number.wrapping_add(1);
        timestamp = timestamp.wrapping_add(samples_per_packet as u32);
    }
    
    info!("Anons gönderimi tamamlandı.");
    Ok(())
}