// Dosya: src/rtp/command.rs
use anyhow::Result;
use bytes::Bytes;
use hound::WavSpec;
use std::net::SocketAddr;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tonic::Status;

pub type SharedLiveStreamSender = std::sync::Arc<
    tokio::sync::Mutex<
        std::option::Option<
            tokio::sync::mpsc::Sender<std::result::Result<AudioFrame, tonic::Status>>,
        >,
    >,
>;

#[derive(Debug)]
pub struct AudioFrame {
    pub data: Bytes,
    pub media_type: String,
}

#[derive(Debug)]
pub struct RecordingSession {
    pub output_uri: String,
    pub spec: WavSpec,
    pub rx_buffer: Vec<i16>,
    pub tx_buffer: Vec<i16>,
    pub call_id: String,
    pub trace_id: String,
    pub max_reached_warned: bool,
}

#[derive(Debug)]
pub enum RtpCommand {
    PlayAudioUri {
        audio_uri: String,
        candidate_target_addr: SocketAddr,
        cancellation_token: CancellationToken,
        responder: Option<oneshot::Sender<Result<()>>>,
    },
    StopAudio,
    StartLiveAudioStream {
        stream_sender: mpsc::Sender<Result<AudioFrame, Status>>,
        target_sample_rate: Option<u32>,
    },
    StopLiveAudioStream,
    EnableEchoTest,
    DisableEchoTest,
    SetTargetAddress {
        target: SocketAddr,
    },
    StartPermanentRecording(RecordingSession),
    StopPermanentRecording {
        responder: oneshot::Sender<Result<String, String>>,
    },
    Shutdown,
}
