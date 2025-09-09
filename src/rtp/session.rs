use crate::config::AppConfig;
// DÜZELTME: Eksik olan 'AudioCodec' import edildi.
use crate::rtp::codecs::{self, AudioCodec};
use crate::rtp::command::{AudioFrame, RtpCommand};
use crate::rtp::stream::send_rtp_stream;
use crate::state::AppState;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio::task::spawn_blocking;
use tokio_util::sync::CancellationToken;
use tonic::Status;
// DÜZELTME: Kullanılmayan 'error' importu kaldırıldı.
use tracing::{info, instrument};
use super::command::RecordingSession;
use crate::rtp::session_utils::{finalize_and_save_recording, load_and_resample_samples_from_uri};
use crate::metrics::ACTIVE_SESSIONS;
use metrics::gauge;
use webrtc_util::marshal::Unmarshal;

pub struct RtpSessionConfig {
    pub app_state: AppState,
    pub app_config: Arc<AppConfig>,
    pub port: u16,
}

#[derive(Debug)]
struct PlaybackJob {
    audio_uri: String,
    target_addr: SocketAddr,
    cancellation_token: CancellationToken,
}

async fn handle_incoming_rtp_packet(
    packet_data: Vec<u8>,
    remote_addr: SocketAddr,
    actual_remote_addr: Arc<Mutex<Option<SocketAddr>>>,
    outbound_codec: Arc<Mutex<Option<AudioCodec>>>,
    live_stream_sender: Arc<Mutex<Option<mpsc::Sender<Result<AudioFrame, Status>>>>>,
    permanent_recording_session: Arc<Mutex<Option<RecordingSession>>>,
) {
    if actual_remote_addr.lock().await.is_none() {
        *actual_remote_addr.lock().await = Some(remote_addr);
        info!(%remote_addr, "İlk RTP paketi alındı, hedef adres doğrulandı.");
    }
    
    let maybe_processed = spawn_blocking(move || {
        let mut packet_buf = &packet_data[..];
        let packet = rtp::packet::Packet::unmarshal(&mut packet_buf).ok()?;
        let codec = codecs::AudioCodec::from_rtp_payload_type(packet.header.payload_type).ok()?;
        let samples = codecs::decode_g711_to_lpcm16(&packet.payload, codec).ok()?;
        Some((samples, codec))
    }).await.ok().flatten();

    if let Some((samples_16khz, codec)) = maybe_processed {
        if outbound_codec.lock().await.is_none() {
            *outbound_codec.lock().await = Some(codec);
        }
        
        let mut sender_guard = live_stream_sender.lock().await;
        if let Some(sender) = &*sender_guard {
            if !sender.is_closed() {
                let mut bytes = Vec::with_capacity(samples_16khz.len() * 2);
                for &sample in &samples_16khz {
                    bytes.extend_from_slice(&sample.to_le_bytes());
                }
                let frame = AudioFrame { data: bytes.into(), media_type: "audio/L16;rate=16000".to_string() };
                if sender.send(Ok(frame)).await.is_err() {
                    // Receiver dropped, do nothing. We'll clean it up below.
                }
            }
        }
        drop(sender_guard);

        let mut sender_guard = live_stream_sender.lock().await;
        if sender_guard.as_ref().map_or(false, |s| s.is_closed()) {
            *sender_guard = None;
        }
        drop(sender_guard);

        if let Some(session) = &mut *permanent_recording_session.lock().await {
            session.inbound_samples.extend_from_slice(&samples_16khz);
        }
    }
}


#[instrument(skip_all, fields(rtp_port = config.port))]
pub async fn rtp_session_handler(
    socket: Arc<tokio::net::UdpSocket>,
    mut command_rx: mpsc::Receiver<RtpCommand>,
    config: RtpSessionConfig,
) {
    info!("Yeni RTP oturumu dinleyicisi başlatıldı (Fire-and-Forget Mimarisi).");
    
    let live_stream_sender = Arc::new(Mutex::new(None));
    let permanent_recording_session = Arc::new(Mutex::new(None));
    let actual_remote_addr = Arc::new(Mutex::new(None));
    let outbound_codec = Arc::new(Mutex::new(None));

    let mut playback_queue: VecDeque<PlaybackJob> = VecDeque::new();
    let mut is_playing = false;
    let (playback_finished_tx, mut playback_finished_rx) = mpsc::channel::<()>(1);

    let mut buf = [0u8; 2048];

    loop {
        tokio::select! {
            Some(command) = command_rx.recv() => {
                match command {
                    RtpCommand::PlayAudioUri { audio_uri, candidate_target_addr, cancellation_token } => {
                        let addr = actual_remote_addr.lock().await.unwrap_or(candidate_target_addr);
                        let job = PlaybackJob { audio_uri, target_addr: addr, cancellation_token };
                        if !is_playing {
                            is_playing = true;
                            start_playback(job, &config, socket.clone(), playback_finished_tx.clone(), permanent_recording_session.clone(), outbound_codec.clone()).await;
                        } else {
                            playback_queue.push_back(job);
                        }
                    },
                    RtpCommand::StopAudio => {
                        if let Some(token) = playback_queue.front().map(|j| j.cancellation_token.clone()) { token.cancel(); }
                        playback_queue.clear();
                    },
                    RtpCommand::StartLiveAudioStream { stream_sender, .. } => {
                        *live_stream_sender.lock().await = Some(stream_sender);
                    },
                    RtpCommand::StartPermanentRecording(session) => {
                        *permanent_recording_session.lock().await = Some(session);
                    },
                    RtpCommand::StopLiveAudioStream => *live_stream_sender.lock().await = None,
                    RtpCommand::StopPermanentRecording { responder } => {
                        if let Some(session) = permanent_recording_session.lock().await.take() {
                            let uri = session.output_uri.clone();
                            let result = finalize_and_save_recording(session, config.app_state.clone()).await;
                            let _ = responder.send(result.map(|_| uri).map_err(|e| e.to_string()));
                        } else {
                            let _ = responder.send(Err("Kayıt bulunamadı".to_string()));
                        }
                    },
                    RtpCommand::Shutdown => break,
                }
            },
            result = socket.recv_from(&mut buf) => {
                if let Ok((len, remote_addr)) = result {
                    let packet_data = buf[..len].to_vec();
                    tokio::spawn(handle_incoming_rtp_packet(
                        packet_data,
                        remote_addr,
                        actual_remote_addr.clone(),
                        outbound_codec.clone(),
                        live_stream_sender.clone(),
                        permanent_recording_session.clone(),
                    ));
                }
            },
            Some(_) = playback_finished_rx.recv() => {
                is_playing = false;
                if let Some(next_job) = playback_queue.pop_front() {
                    is_playing = true;
                    start_playback(next_job, &config, socket.clone(), playback_finished_tx.clone(), permanent_recording_session.clone(), outbound_codec.clone()).await;
                }
            }
        }
    }
    
    if let Some(session) = permanent_recording_session.lock().await.take() {
        tokio::spawn(finalize_and_save_recording(session, config.app_state.clone()));
    }
    config.app_state.port_manager.remove_session(config.port).await;
    config.app_state.port_manager.quarantine_port(config.port).await;
    gauge!(ACTIVE_SESSIONS).decrement(1.0);
    info!("RTP oturumu temizlendi.");
}

async fn start_playback(
    job: PlaybackJob, config: &RtpSessionConfig, socket: Arc<tokio::net::UdpSocket>,
    playback_finished_tx: mpsc::Sender<()>,
    permanent_recording_session: Arc<Mutex<Option<RecordingSession>>>,
    outbound_codec: Arc<Mutex<Option<AudioCodec>>>
) {
    let codec_to_use = outbound_codec.lock().await.unwrap_or(AudioCodec::Pcmu);
    let samples_to_play = match load_and_resample_samples_from_uri(&job.audio_uri, &config.app_state, &config.app_config).await {
        Ok(s) => Some(s),
        Err(_e) => {
            let _ = playback_finished_tx.send(()).await;
            return;
        }
    };

    if let Some(samples_16khz) = samples_to_play {
        if let Some(session) = &mut *permanent_recording_session.lock().await {
            session.inbound_samples.extend_from_slice(&samples_16khz);
        }
        tokio::spawn(async move {
            let _ = send_rtp_stream(&socket, job.target_addr, &samples_16khz, job.cancellation_token, codec_to_use).await;
            let _ = playback_finished_tx.send(()).await;
        });
    } else {
        let _ = playback_finished_tx.send(()).await;
    }
}