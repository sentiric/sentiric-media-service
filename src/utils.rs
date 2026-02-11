// sentiric-media-service/src/utils.rs
use std::net::SocketAddr;
use tokio::net::UdpSocket;
use tokio::time::{sleep, Duration};
use tracing::{debug, warn};

/// Telefon numaralarını KVKK uyumlu şekilde maskeler
/// 905548777858 -> 90554***58
pub fn mask_pii(input: &str) -> String {
    let cleaned: String = input.chars().filter(|c| c.is_ascii_digit()).collect();
    if cleaned.len() < 10 {
        return "****".to_string();
    }
    format!("{}***{}", &cleaned[..5], &cleaned[cleaned.len() - 2..])
}

pub fn extract_uri_scheme(uri: &str) -> &str {
    if let Some(scheme_end) = uri.find(':') {
        &uri[..scheme_end]
    } else {
        "unknown"
    }
}

pub async fn send_hole_punch_packet(socket: &UdpSocket, target: SocketAddr, count: usize) {
    let silence_packet = [0u8; 160]; 
    for i in 0..count {
        if let Err(e) = socket.send_to(&silence_packet, target).await {
            warn!("Hole Punching failed: {}", e);
            break;
        }
        debug!("Hole Punching sent ({}/{}) -> {}", i + 1, count, target);
        sleep(Duration::from_millis(50)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mask_pii() {
        assert_eq!(mask_pii("905548777858"), "90554***58");
        assert_eq!(mask_pii("05548777858"), "05548***58");
        assert_eq!(mask_pii("123"), "****");
    }
}