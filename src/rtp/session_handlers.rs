// Dosya: sentiric-media-service/src/rtp/session_handlers.rs
use super::command::{RecordingSession, RtpCommand};
use super::session::RtpSessionConfig;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{error, info, Instrument};

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
    live_stream_sender: &crate::rtp::command::SharedLiveStreamSender,
    recording_session: &Arc<Mutex<Option<RecordingSession>>>,
    playback_queue: &mut std::collections::VecDeque<PlaybackJob>,
    is_playing: &mut bool,
    echo_mode: &mut bool,
    config: &RtpSessionConfig,
    egress_tx: &mpsc::Sender<Vec<i16>>,
    finished_tx: &mpsc::Sender<()>,
    known_target: &mut Option<SocketAddr>,
    endpoint: &sentiric_rtp_core::RtpEndpoint,
    call_id: &str,
) -> bool {
    match command {
        RtpCommand::PlayAudioUri {
            audio_uri,
            candidate_target_addr,
            cancellation_token,
            responder,
        } => {
            *known_target = Some(candidate_target_addr);
            let target = endpoint
                .get_target()
                .or(*known_target)
                .unwrap_or(candidate_target_addr);

            let job = PlaybackJob {
                audio_uri,
                target_addr: target,
                cancellation_token,
                responder,
            };

            if !*is_playing {
                *is_playing = true;
                start_playback(job, config, egress_tx.clone(), finished_tx.clone(), call_id).await;
            } else {
                playback_queue.push_back(job);
            }
        }
        RtpCommand::EnableEchoTest => {
            info!(event = "ECHO_MODE_ENABLED", sip.call_id = %call_id, "🔊 Native Echo Reflex AKTİFLEŞTİRİLDİ. Loopback başlıyor.");
            *echo_mode = true;
        }
        RtpCommand::DisableEchoTest => {
            info!(event = "ECHO_MODE_DISABLED", sip.call_id = %call_id, "🔇 Native Echo Reflex KAPATILDI.");
            *echo_mode = false;
        }
        RtpCommand::StartLiveAudioStream { stream_sender, .. } => {
            let mut guard = live_stream_sender.lock().await;
            *guard = Some(stream_sender);
        }
        RtpCommand::StopLiveAudioStream => {
            let mut guard = live_stream_sender.lock().await;
            *guard = None;
        }
        RtpCommand::StartPermanentRecording(session) => {
            let mut guard = recording_session.lock().await;
            *guard = Some(session);
        }
        RtpCommand::StopPermanentRecording { responder } => {
            let mut guard = recording_session.lock().await;
            if let Some(session) = guard.take() {
                let app_state = config.app_state.clone();
                let span = tracing::Span::current();
                tokio::spawn(
                    async move {
                        let res = crate::rtp::session_utils::finalize_and_save_recording(
                            session, app_state,
                        )
                        .await;
                        let _ = responder.send(
                            res.map(|_| "Success".to_string())
                                .map_err(|e| e.to_string()),
                        );
                    }
                    .instrument(span),
                );
            }
        }
        RtpCommand::Shutdown => return true,
        RtpCommand::SetTargetAddress { target } => {
            *known_target = Some(target);
        }
        _ => {}
    }
    false
}

pub async fn start_playback(
    mut job: PlaybackJob,
    config: &RtpSessionConfig,
    egress_tx: mpsc::Sender<Vec<i16>>,
    finished_tx: mpsc::Sender<()>,
    call_id: &str,
) {
    let responder = job.responder.take();
    let uri = job.audio_uri.clone();
    let call_id_owned = call_id.to_string();
    let app_state = config.app_state.clone();
    let tenant_id_owned = config.app_config.tenant_id.clone();
    let span = tracing::Span::current();

    match crate::rtp::session_utils::load_and_resample_samples_from_uri(
        &uri,
        &config.app_state,
        &config.app_config,
    )
    .await
    {
        Ok(samples) => {
            if let Some(tx) = responder {
                let _ = tx.send(Ok(()));
            }

            tokio::spawn(async move {
                tracing::info!(event = "MEDIA_PLAYBACK_START", sip.call_id = %call_id_owned, uri = %uri, "🚀 Medya PCM chunk'ları Egress kanalına basılıyor.");

                let mut res_ok = true;
                let mut interval = tokio::time::interval(std::time::Duration::from_millis(20));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

                let mut pre_buffer = 10;

                for chunk in samples.chunks(160) {
                    if job.cancellation_token.is_cancelled() { break; }

                    if pre_buffer > 0 {
                        pre_buffer -= 1;
                    } else {
                        interval.tick().await;
                    }

                    if let Err(e) = egress_tx.send(chunk.to_vec()).await {
                        tracing::debug!(event = "EGRESS_SEND_ERROR", error = %e, "Egress channel closed");
                        res_ok = false;
                        break;
                    }
                }

                if res_ok {
                    if let Some(channel) = &app_state.rabbitmq_publisher {
                        let json_payload = serde_json::json!({ "callId": call_id_owned, "uri": uri }).to_string();
                        let event = sentiric_contracts::sentiric::event::v1::GenericEvent {
                            event_type: "call.media.playback.finished".to_string(),
                            trace_id: call_id_owned.clone(),
                            timestamp: Some(prost_types::Timestamp::from(std::time::SystemTime::now())),
                            tenant_id: tenant_id_owned,
                            payload_json: json_payload,
                        };
                        use lapin::{options::BasicPublishOptions, BasicProperties};
                        use prost::Message;
                        let _ = channel.basic_publish(
                            crate::rabbitmq::EXCHANGE_NAME,
                            "call.media.playback.finished",
                            BasicPublishOptions::default(),
                            &event.encode_to_vec(),
                            BasicProperties::default()
                        ).await;
                    }
                }

                let _ = finished_tx.try_send(());
            }.instrument(span));
        }
        Err(e) => {
            tracing::error!(event = "MEDIA_PLAYBACK_ERROR", error = %e, "Medya oynatma hatası");
            if let Some(tx) = responder {
                let _ = tx.send(Err(anyhow::anyhow!("Playback error: {}", e)));
            }
            let _ = finished_tx.try_send(());
        }
    }
}
