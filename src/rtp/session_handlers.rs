// sentiric-media-service/src/rtp/session_handlers.rs
use super::command::{RtpCommand, AudioFrame, RecordingSession};
use super::session_utils::load_and_resample_samples_from_uri; 
use super::stream::send_rtp_stream;
use super::session::RtpSessionConfig;
use std::sync::Arc;
use std::net::SocketAddr;
use tokio::sync::{mpsc, oneshot, Mutex};
// use tokio::task; // [CLEANUP] Unused import kaldırıldı.
use tracing::error;

#[derive(Debug)]
pub struct PlaybackJob {
    pub audio_uri: String,
    pub target_addr: SocketAddr,
    pub cancellation_token: tokio_util::sync::CancellationToken,
    pub responder: Option<oneshot::Sender<anyhow::Result<()>>>,
}

/// Gelen komutları işleyen ve oturum durumunu güncelleyen ana mantık.
#[allow(clippy::too_many_arguments)]
pub async fn handle_command(
    command: RtpCommand,
    _rtp_port: u16,
    _live_stream_sender: &Arc<Mutex<Option<mpsc::Sender<Result<AudioFrame, tonic::Status>>>>>,
    _recording_session: &Arc<Mutex<Option<RecordingSession>>>,
    outbound_stream_rx: &mut Option<mpsc::Receiver<Vec<u8>>>,
    is_streaming: &mut bool,
    playback_queue: &mut std::collections::VecDeque<PlaybackJob>,
    is_playing: &mut bool,
    config: &RtpSessionConfig,
    socket: &Arc<tokio::net::UdpSocket>,
    finished_tx: &mpsc::Sender<()>,
    known_target: &mut Option<SocketAddr>,
    endpoint: &sentiric_rtp_core::RtpEndpoint,
) -> bool {
    match command {
        RtpCommand::PlayAudioUri { audio_uri, candidate_target_addr, cancellation_token, responder } => {
            let target = endpoint.get_target().or(*known_target).unwrap_or(candidate_target_addr);
            let job = PlaybackJob { audio_uri, target_addr: target, cancellation_token, responder };
            if !*is_playing {
                *is_playing = true;
                start_playback(job, config, socket.clone(), finished_tx.clone()).await;
            } else { 
                playback_queue.push_back(job); 
            }
        },
        RtpCommand::StartOutboundStream { audio_rx } => {
            *outbound_stream_rx = Some(audio_rx);
            *is_streaming = true;
        },
        RtpCommand::StopOutboundStream => { 
            *is_streaming = false; 
        },
        RtpCommand::Shutdown => return true,
        _ => {}
    }
    false
}

pub async fn start_playback(
    job: PlaybackJob, 
    config: &RtpSessionConfig, 
    socket: Arc<tokio::net::UdpSocket>, 
    finished_tx: mpsc::Sender<()>
) {
    let responder = job.responder;
    match load_and_resample_samples_from_uri(&job.audio_uri, &config.app_state, &config.app_config).await {
        Ok(samples) => {
            tokio::spawn(async move {
                let res = send_rtp_stream(
                    &socket, 
                    job.target_addr, 
                    &samples, 
                    job.cancellation_token, 
                    crate::rtp::codecs::AudioCodec::Pcmu
                ).await;
                if let Some(tx) = responder { 
                    let _ = tx.send(res.map_err(anyhow::Error::from)); 
                }
                let _ = finished_tx.try_send(());
            });
        },
        Err(e) => {
            error!("Playback error: {}", e);
            if let Some(tx) = responder { 
                let _ = tx.send(Err(anyhow::anyhow!(e.to_string()))); 
            }
            let _ = finished_tx.try_send(());
        }
    }
}