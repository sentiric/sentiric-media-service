// File: src/rtp/command.rs
use std::net::SocketAddr;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tonic::Status;
use bytes::Bytes;
use hound::WavSpec;

#[derive(Debug)]
pub struct AudioFrame {
    pub data: Bytes,
    pub media_type: String, // Örn: "audio/L16;rate=16000"
}

// --- DEĞİŞİKLİK BURADA: Gerekli alanlar eklendi ---
#[derive(Debug)]
pub struct RecordingSession {
    pub output_uri: String,
    pub spec: WavSpec,
    pub samples: Vec<i16>,
    pub call_id: String,
    pub trace_id: String,
}
// --- DEĞİŞİKLİK SONU ---

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
        // Bu kanal, kaydetme işleminin sonucunu (başarılı URI veya hata) geri bildirecek.
        responder: oneshot::Sender<Result<String, String>>,
    },
    Shutdown,
}