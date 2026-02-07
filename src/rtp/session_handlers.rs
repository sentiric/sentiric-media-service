// sentiric-media-service/src/rtp/session_handlers.rs
use super::command::{RtpCommand, AudioFrame, RecordingSession};
use super::session_utils::load_and_resample_samples_from_uri; 
use super::stream::send_rtp_stream;
use super::session::RtpSessionConfig;
use std::sync::Arc;
use std::net::SocketAddr;
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{info, error, debug};

#[derive(Debug)]
pub struct PlaybackJob {
    pub audio_uri: String,
    pub target_addr: SocketAddr,
    pub cancellation_token: tokio_util::sync::CancellationToken,
    pub responder: Option<oneshot::Sender<anyhow::Result<()>>>,
}

/// handle_command: RtpSession döngüsü içinden gelen komutları işler.
/// Geri dönüş değeri 'true' ise oturum sonlandırılmalıdır.
#[allow(clippy::too_many_arguments)]
pub async fn handle_command(
    command: RtpCommand,
    _rtp_port: u16,
    live_stream_sender: &Arc<Mutex<Option<mpsc::Sender<Result<AudioFrame, tonic::Status>>>>>,
    recording_session: &Arc<Mutex<Option<RecordingSession>>>,
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
                debug!("⌛ Playback busy, queuing job: {}", job.audio_uri);
                playback_queue.push_back(job); 
            }
        },

        RtpCommand::StartOutboundStream { audio_rx } => {
            info!("🎤 Starting Outbound Audio Injector (TTS Pipeline)");
            *outbound_stream_rx = Some(audio_rx);
            *is_streaming = true;
        },

        RtpCommand::StopOutboundStream => { 
            info!("🔇 Stopping Outbound Audio Injector");
            *is_streaming = false; 
            *outbound_stream_rx = None;
        },

        RtpCommand::StartLiveAudioStream { stream_sender, .. } => {
            info!("👂 Starting Inbound Live Stream (STT Pipeline)");
            let mut guard = live_stream_sender.lock().await;
            *guard = Some(stream_sender);
        },

        RtpCommand::StopLiveAudioStream => {
            let mut guard = live_stream_sender.lock().await;
            *guard = None;
        },

        RtpCommand::StartPermanentRecording(session) => {
            info!(uri = %session.output_uri, "⏺️  Call recording started.");
            let mut guard = recording_session.lock().await;
            *guard = Some(session);
        },

        RtpCommand::StopPermanentRecording { responder } => {
            info!("💾 Finalizing call recording...");
            let mut guard = recording_session.lock().await;
            if let Some(session) = guard.take() {
                let app_state = config.app_state.clone();
                tokio::spawn(async move {
                    let res = crate::rtp::session_utils::finalize_and_save_recording(session, app_state).await;
                    let _ = responder.send(res.map(|_| "Success".to_string()).map_err(|e| e.to_string()));
                });
            } else {
                let _ = responder.send(Err("No active recording session found".to_string()));
            }
        },

        RtpCommand::Shutdown => {
            info!("🛑 Shutdown command received for session.");
            return true;
        },

        _ => {
            debug!("ℹ️ Command handled in session root or ignored: {:?}", command);
        }
    }
    false
}

/// start_playback: Bir anonsun çalınma sürecini başlatır.
pub async fn start_playback(
    job: PlaybackJob, 
    config: &RtpSessionConfig, 
    socket: Arc<tokio::net::UdpSocket>, 
    finished_tx: mpsc::Sender<()>
) {
    let responder = job.responder;
    let uri = job.audio_uri.clone();

    match load_and_resample_samples_from_uri(&uri, &config.app_state, &config.app_config).await {
        Ok(samples) => {
            tokio::spawn(async move {
                let res = send_rtp_stream(
                    &socket, 
                    job.target_addr, 
                    &samples, 
                    job.cancellation_token, 
                    crate::rtp::codecs::AudioCodec::Pcmu // Default outgoing diagnostic codec
                ).await;
                
                if let Some(tx) = responder { 
                    let _ = tx.send(res.map_err(anyhow::Error::from)); 
                }
                let _ = finished_tx.try_send(());
            });
        },
        Err(e) => {
            error!("❌ Playback initialization error for {}: {}", uri, e);
            if let Some(tx) = responder { 
                let _ = tx.send(Err(anyhow::anyhow!(e.to_string()))); 
            }
            let _ = finished_tx.try_send(());
        }
    }
}