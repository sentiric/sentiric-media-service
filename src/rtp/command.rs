// File: src/rtp/command.rs (GÜNCELLENMİŞ)

use std::net::SocketAddr;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tonic::Status;
use bytes::Bytes;
use hound::WavSpec;

#[derive(Debug)]
pub struct AudioFrame {
    pub data: Bytes,
    pub media_type: String, // Örn: "audio/L16;rate=16000"
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
    // YENİ KOMUT: Canlı ses akışını bu kanala yönlendir.
    StartLiveAudioStream {
        stream_sender: mpsc::Sender<Result<AudioFrame, Status>>,
        target_sample_rate: Option<u32>,
    },
    // YENİ KOMUT: Canlı ses akışını durdur.
    StopLiveAudioStream,
    StartPermanentRecording(RecordingSession),
    StopPermanentRecording,
    Shutdown,
}