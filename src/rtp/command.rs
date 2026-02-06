// sentiric-media-service/src/rtp/command.rs
use anyhow::Result;
use bytes::Bytes;
use hound::WavSpec;
use std::net::SocketAddr;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tonic::Status;

#[derive(Debug)]
pub struct AudioFrame {
    pub data: Bytes,
    pub media_type: String,
}

#[derive(Debug)]
pub struct RecordingSession {
    pub output_uri: String,
    pub spec: WavSpec,
    pub mixed_samples_16khz: Vec<i16>,
    pub call_id: String,
    pub trace_id: String,
}

#[derive(Debug)]
pub enum RtpCommand {
    // Mevcut dosya/URI oynatma komutu
    PlayAudioUri {
        audio_uri: String,
        candidate_target_addr: SocketAddr,
        cancellation_token: CancellationToken,
        responder: Option<oneshot::Sender<Result<()>>>,
    },
    StopAudio,
    
    // Canlı ses dinleme (Inbound -> gRPC)
    StartLiveAudioStream {
        stream_sender: mpsc::Sender<Result<AudioFrame, Status>>,
        target_sample_rate: Option<u32>,
    },
    StopLiveAudioStream,
    
    // --- YENİ KRİTİK KOMUTLAR ---
    // Media Stream'den gelen ham ses verisi (gRPC -> Outbound)
    StartOutboundStream {
        audio_rx: mpsc::Receiver<Vec<u8>>,
    },
    StopOutboundStream,
    
    // PlayAudio'dan gelen adresi kaydetme (Latching'i tetiklemek için)
    SetTargetAddress {
        target: SocketAddr,
    },
    // Hole Punching'i manuel tetikleme
    HolePunching {
        target_addr: SocketAddr,
    },
    
    // Kalıcı kayıt işlemleri
    StartPermanentRecording(RecordingSession),
    StopPermanentRecording {
        responder: oneshot::Sender<Result<String, String>>,
    },
    
    Shutdown,
}