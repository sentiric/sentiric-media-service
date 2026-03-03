// src/rtp/command.rs
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
    // [DEĞİŞİKLİK]: İsim genelleştirildi. Artık hem 8k hem 16k tutabilir.
    pub audio_buffer: Vec<i16>,
    pub call_id: String,
    pub trace_id: String,
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
    StartOutboundStream {
        audio_rx: mpsc::Receiver<Vec<u8>>,
    },
    StopOutboundStream,
    EnableEchoTest,   
    DisableEchoTest,
    SetTargetAddress { target: SocketAddr },
    HolePunching { target_addr: SocketAddr },
    StartPermanentRecording(RecordingSession),
    StopPermanentRecording { responder: oneshot::Sender<Result<String, String>> },
    Shutdown,
}