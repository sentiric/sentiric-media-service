use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use anyhow::Result;
use rand::Rng;
use tokio::net::UdpSocket;
use tokio::time::sleep;
use tracing::{error, info, instrument};
use rtp::header::Header;
use rtp::packet::Packet;
use webrtc_util::marshal::Marshal;

use crate::audio::{self, AudioCache, linear_to_ulaw};
use crate::config::AppConfig;

#[instrument(skip_all, fields(remote = %target_addr, file = %audio_id))]
pub async fn send_announcement(
    sock: Arc<UdpSocket>, 
    target_addr: SocketAddr, 
    audio_id: String, 
    cache: AudioCache, 
    config: Arc<AppConfig>
) {
    info!("Anons gönderimi başlıyor...");
    if let Err(e) = send_announcement_internal(sock, target_addr, &audio_id, &cache, &config).await {
        error!(error = ?e, "Anons gönderimi sırasında bir hata oluştu.");
    }
}

async fn send_announcement_internal(
    sock: Arc<UdpSocket>, 
    target_addr: SocketAddr, 
    audio_id: &str, 
    cache: &AudioCache, 
    config: &AppConfig
) -> Result<()> {
    let samples = audio::load_or_get_from_cache(cache, audio_id, &config.assets_base_path).await?;
    
    let ssrc: u32 = rand::thread_rng().gen();
    let mut sequence_number: u16 = rand::thread_rng().gen();
    let mut timestamp: u32 = rand::thread_rng().gen();
    const SAMPLES_PER_PACKET: usize = 160;

    for chunk in samples.chunks(SAMPLES_PER_PACKET) {
        let packet = Packet {
            header: Header { version: 2, payload_type: 0, sequence_number, timestamp, ssrc, ..Default::default() },
            payload: chunk.iter().map(|&s| linear_to_ulaw(s)).collect(),
        };
        sock.send_to(&packet.marshal()?, target_addr).await?;
        sequence_number = sequence_number.wrapping_add(1);
        timestamp = timestamp.wrapping_add(SAMPLES_PER_PACKET as u32);
        sleep(Duration::from_millis(20)).await;
    }
    info!("Anons gönderimi tamamlandı.");
    Ok(())
}