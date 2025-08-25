use std::net::SocketAddr;
use tokio::sync::mpsc;
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
    pub samples: Vec<i16>,
}

#[derive(Debug)]
pub enum RtpCommand {
    PlayAudioUri {
        audio_uri: String,
        candidate_target_addr: SocketAddr,
        cancellation_token: CancellationToken,
    },
    StopAudio,
    StartRecording {
        stream_sender: mpsc::Sender<Result<AudioFrame, Status>>,
        target_sample_rate: Option<u32>,
    },
    StartPermanentRecording(RecordingSession),
    StopPermanentRecording,
    Shutdown,
}