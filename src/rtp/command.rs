// sentiric-media-service/src/rtp/command.rs
use std::net::SocketAddr;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tonic::Status;
use bytes::Bytes;
use hound::WavSpec;

#[derive(Debug)]
pub struct AudioFrame {
    pub data: Bytes,
    pub media_type: String,
}

#[derive(Debug)]
pub struct RecordingSession {
    pub output_uri: String,
    pub spec: WavSpec,
    // --- DEĞİŞİKLİK BURADA ---
    // Ayrı tamponlar yerine, anlık olarak birleştirilmiş ses örneklerini tutacağız.
    // Bu, 16kHz LPCM formatında olacak.
    pub mixed_samples_16khz: Vec<i16>,
    // --- DEĞİŞİKLİK SONU ---
    pub call_id: String,
    pub trace_id: String,
}

#[derive(Debug)]
pub enum RtpCommand {
    PlayAudioUri {
        audio_uri: String,
        candidate_target_addr: SocketAddr,
        cancellation_token: CancellationToken,
    },
    StopAudio,
    StartLiveAudioStream {
        stream_sender: mpsc::Sender<Result<AudioFrame, Status>>,
        target_sample_rate: Option<u32>,
    },
    StopLiveAudioStream,
    StartPermanentRecording(RecordingSession),
    StopPermanentRecording {
        responder: oneshot::Sender<Result<String, String>>,
    },
    Shutdown,
}