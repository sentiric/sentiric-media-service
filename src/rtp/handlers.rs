// sentiric-media-service/src/rtp/handlers.rs

use super::command::{RtpCommand, AudioFrame};
use super::session_utils::{finalize_and_save_recording, load_and_resample_samples_from_uri};
use super::stream::send_rtp_stream;
use crate::rtp::command::RecordingSession;
use anyhow::Result;
use std::sync::Arc;
use std::net::SocketAddr;
use tokio::sync::{mpsc, Mutex};
use tokio::task;
use tracing::{info, error};

// PlaybackJob, handlers modülüne özel bir iç yapı haline geldi.
#[derive(Debug)]
pub struct PlaybackJob {
    pub audio_uri: String,
    pub target_addr: SocketAddr,
    pub cancellation_token: tokio_util::sync::CancellationToken,
    pub responder: Option<tokio::sync::oneshot::Sender<Result<()>>>,
}

/// Gelen komutları işleyen ve oturum durumunu güncelleyen ana mantık.
#[allow(clippy::too_many_arguments)]
pub async fn handle_command(
    command: RtpCommand,
    rtp_port: u16,
    live_stream_sender: &Arc<Mutex<Option<mpsc::Sender<Result<AudioFrame, tonic::Status>>>>>,
    permanent_recording_session: &Arc<Mutex<Option<RecordingSession>>>,
    outbound_stream_rx: &mut Option<mpsc::Receiver<Vec<u8>>>,
    is_streaming_active: &mut bool,
    playback_queue: &mut std::collections::VecDeque<PlaybackJob>,
    is_playing_file: &mut bool,
    config: &super::session::RtpSessionConfig,
    socket: &Arc<tokio::net::UdpSocket>,
    playback_finished_tx: &mpsc::Sender<()>,
    pre_latch_target: &mut Option<SocketAddr>,
    endpoint: &sentiric_rtp_core::RtpEndpoint
) -> bool { // `true` dönerse döngüden çık (Shutdown)
    
    match command {
        RtpCommand::PlayAudioUri { audio_uri, candidate_target_addr, cancellation_token, responder } => {
            if endpoint.get_target().is_none() {
                *pre_latch_target = Some(candidate_target_addr);
            }
            // Hole Punching
            let target_to_punch = endpoint.get_target().unwrap_or(candidate_target_addr);
            let socket_clone = socket.clone();
            tokio::spawn(async move {
                let silence = vec![0x80u8; 160]; 
                for _ in 0..5 { 
                    let _ = socket_clone.send_to(&silence, target_to_punch).await;
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            });
            
            let target = endpoint.get_target().or(*pre_latch_target).unwrap_or(candidate_target_addr);
            let job = PlaybackJob { audio_uri, target_addr: target, cancellation_token, responder };
            
            if !*is_playing_file {
                *is_playing_file = true;
                start_playback(job, config, socket.clone(), playback_finished_tx.clone()).await;
            } else { 
                playback_queue.push_back(job); 
            }
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
                let app_state_clone = config.app_state.clone();
                tokio::spawn(async move { let _ = finalize_and_save_recording(session, app_state_clone).await; });
                let _ = responder.send(Ok(uri));
            } else { 
                let _ = responder.send(Err("Kayıt bulunamadı".to_string())); 
            }
        },
        RtpCommand::Shutdown => { 
            info!(port = rtp_port, "🛑 Shutdown komutu alındı.");
            return true; 
        },
        RtpCommand::StopAudio => { 
            playback_queue.clear(); 
        },
    }
    false // Döngüye devam et
}

// [GÖRÜNÜRLÜK DÜZELTMESİ] Fonksiyonu 'pub' yaparak dışarıya açtık.
pub async fn start_playback(
    job: PlaybackJob, 
    config: &super::session::RtpSessionConfig, 
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
        Err(e) => {
            error!(uri = %job.audio_uri, error = %e, "Anons dosyası yüklenemedi.");
            if let Some(tx) = responder { let _ = tx.send(Err(anyhow::anyhow!(e.to_string()))); }
            let _ = playback_finished_tx.try_send(());
        }
    };
}