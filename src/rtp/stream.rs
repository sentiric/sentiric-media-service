// sentiric-media-service/src/rtp/stream.rs

use crate::rtp::codecs::{self, AudioCodec};
use anyhow::Result;
use rand::Rng;
use sentiric_rtp_core::{RtpHeader, RtpPacket};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn, debug}; // Debug eklendi

pub async fn send_rtp_stream(
    sock: &Arc<UdpSocket>,
    target_addr: SocketAddr,
    samples_16khz: &[i16],
    token: CancellationToken,
    target_codec: AudioCodec,
) -> Result<()> {
    // 1. Encode
    let encoded_payload = codecs::encode_lpcm16_to_g711(samples_16khz, target_codec)?;
    
    info!(
        source_samples = samples_16khz.len(),
        encoded_bytes = encoded_payload.len(),
        target = %target_addr,
        "Ses akışı (Anons) başlatılıyor."
    );

    // --- DEFANSİF KONTROL: Geçersiz Adres ---
    // Eğer hedef adres 0.0.0.0 veya port 0 ise, paket gönderme.
    // Bu durum, Latching henüz gerçekleşmediyse ve B2BUA dummy IP gönderdiyse oluşur.
    let is_valid_target = !target_addr.ip().is_unspecified() && target_addr.port() != 0;
    if !is_valid_target {
        warn!("⚠️ Hedef adres geçersiz (0.0.0.0:0). Latching bekleniyor veya B2BUA SDP bilgisini göndermedi. Paketler atlanacak.");
    }

    let ssrc: u32 = rand::thread_rng().gen();
    let mut sequence_number: u16 = rand::thread_rng().gen();
    let mut timestamp: u32 = rand::thread_rng().gen();
    
    const SAMPLES_PER_PACKET: usize = 160;

    let rtp_payload_type = match target_codec {
        AudioCodec::Pcmu => 0,
        AudioCodec::Pcma => 8,
    };

    let mut ticker = tokio::time::interval(Duration::from_millis(20));

    for chunk in encoded_payload.chunks(SAMPLES_PER_PACKET) {
        tokio::select! {
            biased;
            _ = token.cancelled() => {
                info!("RTP akışı dışarıdan iptal edildi.");
                return Ok(());
            }
            _ = ticker.tick() => {
                // Sadece geçerli bir hedef varsa gönderim yap
                if is_valid_target {
                    let header = RtpHeader::new(rtp_payload_type, sequence_number, timestamp, ssrc);
                    let packet = RtpPacket {
                        header,
                        payload: chunk.to_vec(),
                    };
                    
                    if let Err(e) = sock.send_to(&packet.to_bytes(), target_addr).await {
                        // OS Error 22 (EINVAL) almamak için loglayıp geçiyoruz
                        debug!(error = %e, target = %target_addr, "RTP paketi gönderilemedi (Ağ hatası).");
                    }
                }
                
                // Zaman damgası ve sıra numarası, paket gönderilmese bile artmalı (Silence Suppression mantığı)
                sequence_number = sequence_number.wrapping_add(1);
                timestamp = timestamp.wrapping_add(SAMPLES_PER_PACKET as u32);
            }
        }
    }
    
    if is_valid_target {
        info!("Anons gönderimi tamamlandı.");
    } else {
        warn!("Anons süresi doldu ancak geçerli bir hedefe gönderilemedi (NAT Latching gerçekleşmedi).");
    }
    
    token.cancel();
    Ok(())
}