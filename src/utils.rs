// sentiric-media-service/src/utils.rs

use std::net::SocketAddr;
use tokio::net::UdpSocket;
use tokio::time::{sleep, Duration};
use tracing::{debug, warn};

// Hole Punching için gerekli basit URI şeması ayıklayıcı
pub fn extract_uri_scheme(uri: &str) -> &str {
    if let Some(scheme_end) = uri.find(':') {
        &uri[..scheme_end]
    } else {
        "unknown"
    }
}

/// Hole Punching paketlerini hedef adrese gönderir.
pub async fn send_hole_punch_packet(socket: &UdpSocket, target: SocketAddr, count: usize) {
    // 20ms'lik G.711 PCMU boş paketi. SBC/Proxy'nin NAT delmesi için yeterlidir.
    // 160 bytes of 0x80 (silence in PCMU)
    let silence_packet = [0u8; 160]; 
    
    for i in 0..count {
        if let Err(e) = socket.send_to(&silence_packet, target).await {
            warn!("Hole Punching paketi gönderilemedi: {}", e);
            break;
        }
        debug!("Hole Punching paketi gönderildi ({}/{}) -> {}", i + 1, count, target);
        sleep(Duration::from_millis(50)).await;
    }
}