// ========== FILE: sentiric-media-service/src/rtp/session.rs ==========
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{info, instrument, warn};

use crate::audio::AudioCache;
use crate::config::AppConfig;
use crate::rtp::command::RtpCommand;
use crate::rtp::stream::send_announcement_from_uri;
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
    // Mevcut çalma işlemini iptal etmek için kullanılan token'ı sakla
    let mut current_playback_token: Option<CancellationToken> = None;

    loop {
        tokio::select! {
            biased;
            Some(command) = rx.recv() => {
                match command {
                    RtpCommand::PlayAudioUri { audio_uri, candidate_target_addr, cancellation_token } => {
                        // Eğer zaten bir şey çalıyorsa, önce onu durdur
                        if let Some(token) = current_playback_token.take() {
                            warn!("Yeni bir PlayAudio komutu alındı, mevcut çalma işlemi iptal ediliyor.");
                            token.cancel();
                        }
                        
                        // Yeni çalma işleminin token'ını sakla
                        current_playback_token = Some(cancellation_token.clone());

                        let target = actual_remote_addr.unwrap_or(candidate_target_addr);
                        tokio::spawn(send_announcement_from_uri(
                            socket.clone(),
                            target,
                            audio_uri,
                            audio_cache.clone(),
                            config.clone(),
                            cancellation_token, // Token'ı stream fonksiyonuna ver
                        ));
                    },
                    RtpCommand::StopAudio => {
                        if let Some(token) = current_playback_token.take() {
                            info!("StopAudio komutu alındı, mevcut çalma işlemi iptal ediliyor.");
                            token.cancel();
                        }
                    },
                    RtpCommand::Shutdown => {
                        info!("Shutdown komutu alındı, oturum sonlandırılıyor.");
                        if let Some(token) = current_playback_token.take() {
                            token.cancel();
                        }
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