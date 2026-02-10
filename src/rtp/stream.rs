// sentiric-media-service/src/rtp/stream.rs
use crate::rtp::codecs::{self, AudioCodec};
use anyhow::Result;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;
use tracing::{info, debug};

// CORE v1.3.6
use sentiric_rtp_core::{Pacer, RtpHeader, RtpPacket};

pub async fn send_rtp_stream(
    sock: &Arc<UdpSocket>,
    target_addr: SocketAddr,
    samples_16khz: &[i16],
    token: CancellationToken,
    target_codec: AudioCodec,
) -> Result<()> {
    let encoded_payload = codecs::encode_lpcm16_to_rtp(samples_16khz, target_codec)?;
    
    info!(target = %target_addr, "🚀 Precision stream starting. Codec: {:?}", target_codec);

    let ssrc: u32 = rand::random();
    let mut sequence_number: u16 = rand::random();
    let mut timestamp: u32 = rand::random();
    
    let rtp_payload_type = target_codec.to_payload_type();

    // [TELECOM COMPLIANCE]: Packetization Time (ptime) Alignment
    // SIP Core artık SDP'de varsayılan olarak "a=ptime:20" gönderiyor.
    // RTP stream'i buna kesinlikle uymalıdır.
    // G.729: 1 frame = 10ms (10 bytes). 20ms = 2 frames (20 bytes).
    // PCMA/U: 1 sample = 0.125ms. 20ms = 160 samples (160 bytes).
    
    let (packet_chunk_size, samples_per_packet) = match target_codec {
        AudioCodec::G729 => (20, 160), // FIXED: Was (10, 80). Now 20ms standard.
        _ => (160, 160),               // PCMA/PCMU 20ms standard.
    };

    // Pacer 20ms'ye ayarlı
    let mut pacer = Pacer::new(20);

    for chunk in encoded_payload.chunks(packet_chunk_size) {
        if token.is_cancelled() { break; }

        pacer.wait(); 

        let header = RtpHeader::new(rtp_payload_type, sequence_number, timestamp, ssrc);
        let packet = RtpPacket { header, payload: chunk.to_vec() };
        
        if let Err(e) = sock.send_to(&packet.to_bytes(), target_addr).await {
            debug!(error = %e, "RTP send fail");
        }
        
        sequence_number = sequence_number.wrapping_add(1);
        timestamp = timestamp.wrapping_add(samples_per_packet as u32);
    }
    
    Ok(())
}