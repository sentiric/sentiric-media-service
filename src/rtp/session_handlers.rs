// sentiric-media-service/src/rtp/session_handlers.rs
use super::command::{RtpCommand, AudioFrame, RecordingSession};
use super::session_utils::load_and_resample_samples_from_uri; 
use super::stream::send_rtp_stream;
use super::session::RtpSessionConfig;
use crate::rabbitmq;
use std::sync::Arc;
use std::net::SocketAddr;
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{info, error, debug};
use lapin::{options::BasicPublishOptions, BasicProperties};

#[derive(Debug)]
pub struct PlaybackJob {
    pub audio_uri: String,
    pub target_addr: SocketAddr,
    pub cancellation_token: tokio_util::sync::CancellationToken,
    pub responder: Option<oneshot::Sender<anyhow::Result<()>>>,
}

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
    call_id: &str, // Added call_id for reporting
) -> bool {
    match command {
        RtpCommand::PlayAudioUri { audio_uri, candidate_target_addr, cancellation_token, responder } => {
            let target = endpoint.get_target().or(*known_target).unwrap_or(candidate_target_addr);
            let _ = socket.send_to(&[0x80; 12], target).await;
            
            let job = PlaybackJob { audio_uri, target_addr: target, cancellation_token, responder };
            
            if !*is_playing {
                *is_playing = true;
                start_playback(job, config, socket.clone(), finished_tx.clone(), call_id).await;
            } else { 
                playback_queue.push_back(job); 
            }
        },
        RtpCommand::EnableEchoTest => {
            info!("🔊 Native Echo Reflex ENABLED. Sending aggressive warmer.");
            if let Some(target) = endpoint.get_target().or(*known_target) {
                // [AGGRESSIVE HOLE PUNCHING]: 3 paket göndererek tüneli zorla aç
                for _ in 0..3 {
                    let _ = socket.send_to(&[0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], target).await;
                }
            }
        },
        RtpCommand::StartOutboundStream { audio_rx } => { *outbound_stream_rx = Some(audio_rx); *is_streaming = true; },
        RtpCommand::StopOutboundStream => { *is_streaming = false; *outbound_stream_rx = None; },
        RtpCommand::StartLiveAudioStream { stream_sender, .. } => { let mut guard = live_stream_sender.lock().await; *guard = Some(stream_sender); },
        RtpCommand::StopLiveAudioStream => { let mut guard = live_stream_sender.lock().await; *guard = None; },
        RtpCommand::StartPermanentRecording(session) => { let mut guard = recording_session.lock().await; *guard = Some(session); },
        RtpCommand::StopPermanentRecording { responder } => {
            let mut guard = recording_session.lock().await;
            if let Some(session) = guard.take() {
                let app_state = config.app_state.clone();
                tokio::spawn(async move {
                    let res = crate::rtp::session_utils::finalize_and_save_recording(session, app_state).await;
                    let _ = responder.send(res.map(|_| "Success".to_string()).map_err(|e| e.to_string()));
                });
            }
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
    finished_tx: mpsc::Sender<()>,
    call_id: &str,
) {
    let responder = job.responder;
    let uri = job.audio_uri.clone();
    let call_id_owned = call_id.to_string();
    let app_state = config.app_state.clone();

    match load_and_resample_samples_from_uri(&uri, &config.app_state, &config.app_config).await {
        Ok(samples) => {
            tokio::spawn(async move {
                let res = send_rtp_stream(&socket, job.target_addr, &samples, job.cancellation_token, crate::rtp::codecs::AudioCodec::Pcmu).await;
                
                // [PROFESYONEL PBX ÖZELLİĞİ]: Oynatma bittiğinde RabbitMQ'ya olay fırlat
                if res.is_ok() {
                    if let Some(channel) = &app_state.rabbitmq_publisher {
                        let payload = serde_json::json!({
                            "eventType": "call.media.playback.finished",
                            "callId": call_id_owned,
                            "uri": uri
                        }).to_string();
                        
                        let _ = channel.basic_publish(
                            rabbitmq::EXCHANGE_NAME,
                            "call.media.playback.finished",
                            BasicPublishOptions::default(),
                            payload.as_bytes(),
                            BasicProperties::default(),
                        ).await;
                        debug!("📢 Sent PlaybackFinished event for {}", call_id_owned);
                    }
                }

                if let Some(tx) = responder { let _ = tx.send(res.map_err(anyhow::Error::from)); }
                let _ = finished_tx.try_send(());
            });
        },
        Err(e) => {
            error!("Playback error: {}", e);
            if let Some(tx) = responder { let _ = tx.send(Err(anyhow::anyhow!(e.to_string()))); }
            let _ = finished_tx.try_send(());
        }
    }
}