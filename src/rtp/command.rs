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
    // --- Media Playback ---
    PlayAudioUri {
        audio_uri: String,
        candidate_target_addr: SocketAddr,
        cancellation_token: CancellationToken,
        responder: Option<oneshot::Sender<Result<()>>>,
    },
    StopAudio,
    
    // --- Inbound Audio Monitoring (AI pipeline Feed) ---
    StartLiveAudioStream {
        stream_sender: mpsc::Sender<Result<AudioFrame, Status>>,
        target_sample_rate: Option<u32>,
    },
    StopLiveAudioStream,
    
    // --- Outbound Audio Injector (TTS pipeline Feed) ---
    StartOutboundStream {
        audio_rx: mpsc::Receiver<Vec<u8>>,
    },
    StopOutboundStream,

    // --- TELECOM NATIVE DIAGNOSTICS ---
    // AI Pipeline'ı tamamen bypass eden Native Echo (Refleks) modu.
    EnableEchoTest,   
    DisableEchoTest,

    // --- Latching & Network ---
    SetTargetAddress { target: SocketAddr },
    HolePunching { target_addr: SocketAddr },
    
    // --- Recording ---
    StartPermanentRecording(RecordingSession),
    StopPermanentRecording { responder: oneshot::Sender<Result<String, String>> },
    
    Shutdown,
}