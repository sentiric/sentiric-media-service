use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tracing::{info, instrument};

use crate::audio::AudioCache;
use crate::config::AppConfig;
use crate::rtp::command::RtpCommand;
use crate::rtp::stream::send_announcement_from_uri; // Düzeltme: Yeni fonksiyonu çağır
use crate::state::PortManager;

#[instrument(skip_all, fields(rtp_port = port))]
pub async fn rtp_session_handler(
    socket: Arc<UdpSocket>,
    mut rx: mpsc::Receiver<RtpCommand>,
    port_manager: PortManager,
    audio_cache: AudioCache,
    config: Arc<AppConfig>,
    port: u16,
) {
    info!("Yeni RTP oturumu dinleyicisi başlatıldı.");
    let mut actual_remote_addr: Option<SocketAddr> = None;
    let mut buf = [0u8; 2048];

    loop {
        tokio::select! {
            biased;
            Some(command) = rx.recv() => {
                match command {
                    RtpCommand::PlayAudioUri { audio_uri, candidate_target_addr } => { // Düzeltme
                        let target = actual_remote_addr.unwrap_or(candidate_target_addr);
                        tokio::spawn(send_announcement_from_uri( // Düzeltme
                            socket.clone(),
                            target,
                            audio_uri, // Düzeltme
                            audio_cache.clone(),
                            config.clone(),
                        ));
                    },
                    RtpCommand::Shutdown => {
                        info!("Shutdown komutu alındı, oturum sonlandırılıyor.");
                        break;
                    }
                }
            },
            result = socket.recv_from(&mut buf) => {
                if let Ok((len, addr)) = result {
                    if len > 0 && actual_remote_addr.is_none() {
                        info!(remote = %addr, "İlk RTP paketi alındı, hedef adres doğrulandı.");
                        actual_remote_addr = Some(addr);
                    }
                }
            },
        }
    }
    
    info!("RTP oturumu temizleniyor...");
    port_manager.remove_session(port).await;
    port_manager.quarantine_port(port).await;
}