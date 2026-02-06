// sentiric-media-service/src/rtp/session_handlers.rs

use super::command::{RtpCommand, AudioFrame};
use super::session_utils::load_and_resample_samples_from_uri; 
use super::stream::send_rtp_stream;
use super::session::RtpSessionConfig;
use super::command::RecordingSession;
use anyhow::Result; // Tek başına yeterli, anyhow::anyhow içeriden alınır.
use std::sync::Arc;
use std::net::SocketAddr;
use tokio::sync::{mpsc, oneshot};
use tokio::task;
use tracing::{info, error}; // Sadece kullanılanlar

// PlaybackJob struct'ı (Artık bu modülün yerel yapısıdır)
#[derive(Debug)]
pub struct PlaybackJob {
    pub audio_uri: String,
    pub target_addr: SocketAddr,
    pub cancellation_token: tokio_util::sync::CancellationToken,
    pub responder: Option<oneshot::Sender<anyhow::Result<()>>>,
}

/// Gelen komutları işleyen ve oturum durumunu güncelleyen ana mantık.
#[allow(clippy::too_many_arguments)]
pub async fn handle_session_command(
    command: RtpCommand,
    rtp_port: u16,
    live_stream_sender: &Arc<tokio::sync::Mutex<Option<mpsc::Sender<Result<AudioFrame, tonic::Status>>>>>,
    permanent_recording_session: &Arc<tokio::sync::Mutex<Option<RecordingSession>>>,
    outbound_stream_rx: &mut Option<mpsc::Receiver<Vec<u8>>>,
    is_streaming_active: &mut bool,
    playback_queue: &mut std::collections::VecDeque<PlaybackJob>,
    is_playing_file: &mut bool,
    config: &RtpSessionConfig,
    socket: &Arc<tokio::net::UdpSocket>,
    playback_finished_tx: &mpsc::Sender<()>,
    known_target_addr: &mut Option<SocketAddr>,
    endpoint: &sentiric_rtp_core::RtpEndpoint,
) -> bool {
    
    match command {
        RtpCommand::PlayAudioUri { audio_uri, candidate_target_addr, cancellation_token, responder } => {
            let target = endpoint.get_target().or(*known_target_addr).unwrap_or(candidate_target_addr);
            let job = PlaybackJob { audio_uri, target_addr: target, cancellation_token, responder };
            
            if !*is_playing_file {
                *is_playing_file = true;
                start_playback(job, config, socket.clone(), playback_finished_tx.clone()).await;
            } else { 
                playback_queue.push_back(job); 
            }
        },
        RtpCommand::StopAudio => { 
            playback_queue.clear(); 
        },
        
        RtpCommand::StartOutboundStream { audio_rx } => { 
            info!(port = rtp_port, "🎙️ TTS Outbound Stream Başlatıldı.");
            *outbound_stream_rx = Some(audio_rx);
            *is_streaming_active = true;
        },
        RtpCommand::StopOutboundStream => { 
            info!(port = rtp_port, "🎙️ TTS Outbound Stream Durduruldu.");
            *outbound_stream_rx = None; 
            *is_streaming_active = false;
        },
        
        RtpCommand::StartLiveAudioStream { stream_sender, .. } => { 
            *live_stream_sender.lock().await = Some(stream_sender); 
        },
        RtpCommand::StopLiveAudioStream => { 
            *live_stream_sender.lock().await = None; 
        },
        
        RtpCommand::StartPermanentRecording(mut session) => {
            session.mixed_samples_16khz.reserve(16000 * 60 * 5); 
            *permanent_recording_session.lock().await = Some(session);
        },
        RtpCommand::StopPermanentRecording { responder } => {
            if let Some(session) = permanent_recording_session.lock().await.take() {
                let uri = session.output_uri.clone();
                let _ = responder.send(Ok(uri));
            } else { 
                let _ = responder.send(Err("Kayıt bulunamadı".to_string())); 
            }
        },
        
        RtpCommand::Shutdown => { 
            info!(port = rtp_port, "🛑 Shutdown komutu alındı.");
            return true; 
        },

        RtpCommand::SetTargetAddress { .. } | RtpCommand::HolePunching { .. } => {
            // Unreachable: Bu komutlar session.rs'in üst match bloğunda işlenir.
        }
    }
    false 
}

pub async fn start_playback(
    job: PlaybackJob, 
    config: &RtpSessionConfig, 
    socket: Arc<tokio::net::UdpSocket>,
    playback_finished_tx: mpsc::Sender<()>,
) {
    let responder = job.responder;

    match load_and_resample_samples_from_uri(&job.audio_uri, &config.app_state, &config.app_config).await {
        Ok(samples_16khz) => {
            task::spawn(async move {
                let local_codec = crate::rtp::codecs::AudioCodec::Pcmu; 
                let stream_result = send_rtp_stream(
                    &socket, job.target_addr, &samples_16khz, 
                    job.cancellation_token, local_codec
                ).await;
                
                if let Err(e) = &stream_result {
                    error!("RTP dosya akış hatası: {}", e);
                }
                if let Some(tx) = responder { 
                    let _ = tx.send(stream_result.map_err(anyhow::Error::from)); 
                }
                let _ = playback_finished_tx.try_send(());
            });
        },
        anyhow::Result::Err(e) => {
            error!(uri = %job.audio_uri, error = %e, "Anons dosyası yüklenemedi.");
            if let Some(tx) = responder { let _ = tx.send(Err(anyhow::anyhow!(e.to_string()))); }
            let _ = playback_finished_tx.try_send(());
        }
    };
}