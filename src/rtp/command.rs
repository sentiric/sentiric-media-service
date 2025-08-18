// ========== FILE: sentiric-media-service/src/rtp/command.rs ==========
use std::net::SocketAddr;
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
pub enum RtpCommand {
    PlayAudioUri {
        audio_uri: String,
        candidate_target_addr: SocketAddr,
        // Her çalma işleminin kendine ait bir iptal token'ı olacak
        cancellation_token: CancellationToken,
    },
    // Mevcut çalma işlemini durdurmak için yeni komut
    StopAudio,
    Shutdown,
}