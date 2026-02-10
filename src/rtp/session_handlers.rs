// sentiric-media-service/src/rtp/session_handlers.rs

use super::command::{RtpCommand, AudioFrame, RecordingSession};
use super::session_utils::load_and_resample_samples_from_uri; 
use super::session::RtpSessionConfig;
use crate::rabbitmq;
use std::sync::Arc;
use std::net::SocketAddr;
use tokio::sync::{mpsc, oneshot, Mutex};
// use tokio::net::UdpSocket; // <-- TEMİZLENDİ
use tracing::{info, error, debug};
use lapin::{options::BasicPublishOptions, BasicProperties};

use sentiric_contracts::sentiric::event::v1::GenericEvent;
use prost::Message;
use std::time::SystemTime;

use sentiric_rtp_core::{Pacer, RtpHeader, RtpPacket, AudioProfile};
use crate::rtp::codecs;

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
    call_id: &str, 
) -> bool {
    // ... (İçerik aynı, sadece import temizlendiği için dosya yeniden yazılmalı)
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
                let profile = AudioProfile::default();
                let target_codec_type = profile.preferred_audio_codec();
                let target_codec = codecs::AudioCodec::from_rtp_payload_type(target_codec_type as u8).unwrap();
                
                let res = match codecs::encode_lpcm16_to_rtp(&samples, target_codec) {
                    Ok(encoded_payload) => {
                        info!(target = %job.target_addr, "🚀 Precision stream starting.");
                        let ssrc: u32 = rand::random();
                        let mut sequence_number: u16 = rand::random();
                        let mut timestamp: u32 = rand::random();
                        let rtp_payload_type = target_codec.to_payload_type();
                        
                        let (packet_chunk_size, samples_per_packet) = match target_codec {
                            codecs::AudioCodec::G729 => (20, 160),
                            _ => (160, 160),
                        };

                        let mut pacer = Pacer::new(profile.ptime as u64);

                        for chunk in encoded_payload.chunks(packet_chunk_size) {
                            if job.cancellation_token.is_cancelled() { break; }
                            pacer.wait(); 
                            let header = RtpHeader::new(rtp_payload_type, sequence_number, timestamp, ssrc);
                            let packet = RtpPacket { header, payload: chunk.to_vec() };
                            
                            if let Err(e) = socket.send_to(&packet.to_bytes(), job.target_addr).await {
                                debug!(error = %e, "RTP send fail");
                            }
                            
                            sequence_number = sequence_number.wrapping_add(1);
                            timestamp = timestamp.wrapping_add(samples_per_packet as u32);
                        }
                        Ok(())
                    },
                    Err(e) => Err(anyhow::Error::from(e)),
                };
                
                if res.is_ok() {
                    if let Some(channel) = &app_state.rabbitmq_publisher {
                        let json_payload = serde_json::json!({ "callId": call_id_owned, "uri": uri }).to_string();
                        let event = GenericEvent {
                            event_type: "call.media.playback.finished".to_string(),
                            trace_id: call_id_owned.clone(), 
                            timestamp: Some(prost_types::Timestamp::from(SystemTime::now())),
                            tenant_id: "system".to_string(),
                            payload_json: json_payload,
                        };
                        let _ = channel.basic_publish(rabbitmq::EXCHANGE_NAME, "call.media.playback.finished", 
                            BasicPublishOptions::default(), &event.encode_to_vec(), BasicProperties::default()).await;
                        debug!("📢 Sent PlaybackFinished event (Protobuf) for {}", call_id_owned);
                    }
                }

                if let Some(tx) = responder { let _ = tx.send(res); }
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