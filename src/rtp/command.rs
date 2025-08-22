// File: sentiric-media-service/src/rtp/command.rs
use std::net::SocketAddr;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tonic::Status;
use bytes::Bytes;

#[derive(Debug)]
pub enum RtpCommand {
    PlayAudioUri {
        audio_uri: String,
        candidate_target_addr: SocketAddr,
        cancellation_token: CancellationToken,
    },
    StopAudio,
    StartRecording {
        stream_sender: mpsc::Sender<Result<Bytes, Status>>,
    },
    Shutdown,
}