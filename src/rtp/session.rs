// File: sentiric-media-service/src/rtp/session.rs
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tonic::Status;
use tracing::{debug, info, instrument, warn};
use bytes::Bytes;

use crate::audio::AudioCache;
use crate.config::AppConfig;
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
    let mut current_playback_token: Option<CancellationToken> = None;
    let mut recording_sender: Option<mpsc::Sender<Result<Bytes, Status>>> = None;

    loop {
        tokio::select! {
            biased;
            Some(command) = rx.recv() => {
                match command {
                    RtpCommand::PlayAudioUri { audio_uri, candidate_target_addr, cancellation_token } => {
                        if let Some(token) = current_playback_token.take() {
                            info!("Devam eden bir anons var, iptal ediliyor.");
                            token.cancel();
                        }
                        current_playback_token = Some(cancellation_token.clone());
                        let target = actual_remote_addr.unwrap_or(candidate_target_addr);
                        tokio::spawn(send_announcement_from_uri(socket.clone(), target, audio_uri, audio_cache.clone(), config.clone(), cancellation_token));
                    },
                    RtpCommand::StartRecording { stream_sender } => {
                        info!("Ses kaydı komutu alındı. Gelen RTP paketleri stream edilecek.");
                        recording_sender = Some(stream_sender);
                    },
                    RtpCommand::StopAudio => {
                        if let Some(token) = current_playback_token.take() {
                            info!("Anons çalma komutu dışarıdan durduruldu.");
                            token.cancel();
                        }
                    },
                    RtpCommand::Shutdown => {
                        info!("Shutdown komutu alındı, oturum sonlandırılıyor.");
                        if let Some(token) = current_playback_token.take() { token.cancel(); }
                        if let Some(sender) = recording_sender.take() { drop(sender); }
                        break;
                    }
                }
            },
            result = socket.recv_from(&mut buf) => {
                if let Ok((len, addr)) = result {
                    if len > 12 {
                        if actual_remote_addr.is_none() {
                            info!(remote = %addr, "İlk RTP paketi alındı, hedef adres doğrulandı.");
                            actual_remote_addr = Some(addr);
                        }
                        if let Some(sender) = &recording_sender {
                            let payload = Bytes::copy_from_slice(&buf[12..len]);
                            if sender.send(Ok(payload)).await.is_err() {
                                warn!("Kayıt stream'i istemci tarafından kapatıldı, kayıt durduruluyor.");
                                recording_sender = None;
                            }
                        }
                    }
                }
            },
        }
    }
    
    info!("RTP oturumu temizleniyor...");
    port_manager.remove_session(port).await;
    port_manager.quarantine_port(port).await;
}