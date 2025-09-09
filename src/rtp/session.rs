// src/rtp/session.rs
// RTP oturum yönetimi, RTP paketlerinin işlenmesi, ses anonslarının çalınması ve kayıt işlemleri
use crate::config::AppConfig;
use crate::rtp::codecs::{self, AudioCodec, StatefulResampler}; // StatefulResampler eklendi
use crate::rtp::command::{AudioFrame, RtpCommand};
use crate::rtp::stream::send_rtp_stream;
use crate::state::AppState;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio::task::{self, spawn_blocking};
use tokio_util::sync::CancellationToken;
use tonic::Status;
use tracing::{error, info, instrument, warn};
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

fn process_rtp_payload(
    packet_data: &[u8],
    resampler: &mut StatefulResampler, // Resampler'ı parametre olarak alır
) -> Option<(Vec<i16>, AudioCodec)> {
    let mut packet_buf = &packet_data[..];
    let packet = rtp::packet::Packet::unmarshal(&mut packet_buf).ok()?;
    let codec = codecs::AudioCodec::from_rtp_payload_type(packet.header.payload_type).ok()?;
    
    // Artık stateful resampler'ı kullanıyoruz
    let samples = codecs::decode_g711_to_lpcm16(&packet.payload, codec, resampler).ok()?;
    Some((samples, codec))
}


#[instrument(skip_all, fields(rtp_port = config.port))]
pub async fn rtp_session_handler(
    socket: Arc<tokio::net::UdpSocket>,
    mut command_rx: mpsc::Receiver<RtpCommand>,
    config: RtpSessionConfig,
) {
    info!("Yeni RTP oturumu dinleyicisi başlatıldı.");
    
    let live_stream_sender = Arc::new(Mutex::new(None));
    let permanent_recording_session = Arc::new(Mutex::new(None));
    let actual_remote_addr = Arc::new(Mutex::new(None));
    let outbound_codec = Arc::new(Mutex::new(None));

    // --- EN ÖNEMLİ DEĞİŞİKLİK: Resampler'ı oturum için bir kez oluştur ---
    let inbound_resampler = Arc::new(Mutex::new(
        StatefulResampler::new(8000, 16000).expect("Resampler oluşturulamadı")
    ));

    let mut playback_queue: VecDeque<PlaybackJob> = VecDeque::new();
    let mut is_playing = false;
    let (playback_finished_tx, mut playback_finished_rx) = mpsc::channel::<()>(1);
    
    let (rtp_packet_tx, mut rtp_packet_rx) = mpsc::channel::<(Vec<u8>, SocketAddr)>(256);

    let socket_reader_handle = {
        let socket = socket.clone();
        let rtp_packet_tx = rtp_packet_tx.clone();
        task::spawn(async move {
            let mut buf = [0u8; 2048];
            loop {
                match socket.recv_from(&mut buf).await {
                    Ok((len, remote_addr)) => {
                        if rtp_packet_tx.send((buf[..len].to_vec(), remote_addr)).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        })
    };

    loop {
        tokio::select! {
            Some(command) = command_rx.recv() => {

                match command {
                    RtpCommand::PlayAudioUri { audio_uri, candidate_target_addr, cancellation_token } => {
                        let addr = actual_remote_addr.lock().await.unwrap_or(candidate_target_addr);
                        info!(uri = %audio_uri, "PlayAudio komutu alındı.");
                        let job = PlaybackJob { audio_uri, target_addr: addr, cancellation_token };
                        
                        if !is_playing {
                            is_playing = true;
                            info!("Mevcut anons yok, yeni anonsu hemen başlat.");
                            start_playback(job, &config, socket.clone(), playback_finished_tx.clone(), permanent_recording_session.clone(), outbound_codec.clone()).await;
                        } else {
                            info!("Mevcut anons çalıyor, yeni anonsu kuyruğa ekle.");
                            playback_queue.push_back(job);
                        }
                    },
                    RtpCommand::StartLiveAudioStream { stream_sender, .. } => {
                        info!("Canlı ses akışı (STT) başlatılıyor.");
                        *live_stream_sender.lock().await = Some(stream_sender);
                    },
                    RtpCommand::StartPermanentRecording(session) => {
                        info!(uri = %session.output_uri, "Kalıcı kayıt oturumu başlatılıyor.");
                        *permanent_recording_session.lock().await = Some(session);
                    },
                    RtpCommand::StopPermanentRecording { responder } => {
                        if let Some(session) = permanent_recording_session.lock().await.take() {
                            info!(uri = %session.output_uri, "Kalıcı kayıt durduruluyor ve sonlandırılıyor.");
                            let uri = session.output_uri.clone();
                            let result = finalize_and_save_recording(session, config.app_state.clone()).await;
                            let _ = responder.send(result.map(|_| uri).map_err(|e| e.to_string()));
                        } else {
                            warn!("Durdurulacak aktif bir kayıt bulunamadı.");
                            let _ = responder.send(Err("Kayıt bulunamadı".to_string()));
                        }
                    },
                    RtpCommand::Shutdown => {
                        info!("Shutdown komutu alındı, oturum sonlandırılıyor.");
                        break;
                    },
                    RtpCommand::StopAudio => { info!("StopAudio komutu alındı, anons kuyruğu temizleniyor."); playback_queue.clear(); },
                    RtpCommand::StopLiveAudioStream => { info!("Canlı ses akışı (STT) durduruluyor."); *live_stream_sender.lock().await = None },
                }
            },
            
            Some((packet_data, remote_addr)) = rtp_packet_rx.recv() => {
                if actual_remote_addr.lock().await.is_none() {
                    *actual_remote_addr.lock().await = Some(remote_addr);
                    info!(%remote_addr, "İlk RTP paketi alındı, hedef adres doğrulandı.");
                }

                let resampler_clone = inbound_resampler.clone();
                let processing_result = spawn_blocking(move || {
                    let mut resampler_guard = resampler_clone.blocking_lock();
                    process_rtp_payload(&packet_data, &mut *resampler_guard)
                }).await;

                if let Ok(Some((samples_16khz, codec))) = processing_result {
                    if outbound_codec.lock().await.is_none() {
                        *outbound_codec.lock().await = Some(codec);
                    }

                    if let Some(session) = &mut *permanent_recording_session.lock().await {
                        session.inbound_samples.extend_from_slice(&samples_16khz);
                    }
                    
                    let mut sender_guard = live_stream_sender.lock().await;
                    if let Some(sender) = &*sender_guard {
                        if !sender.is_closed() {
                            let mut bytes = Vec::with_capacity(samples_16khz.len() * 2);
                            for &sample in &samples_16khz {
                                bytes.extend_from_slice(&sample.to_le_bytes());
                            }
                            let frame = AudioFrame { data: bytes.into(), media_type: "audio/L16;rate=16000".to_string() };
                            if sender.try_send(Ok(frame)).is_err() {
                                // Kanal dolu, sorun değil.
                            }
                        } else {
                            *sender_guard = None;
                        }
                    }
                }
            },

            Some(_) = playback_finished_rx.recv() => {
                is_playing = false;
                info!("Bir anonsun çalınması tamamlandı.");
                if let Some(next_job) = playback_queue.pop_front() {
                    is_playing = true;
                    info!(uri = %next_job.audio_uri, "Kuyruktaki bir sonraki anons başlatılıyor.");
                    start_playback(next_job, &config, socket.clone(), playback_finished_tx.clone(), permanent_recording_session.clone(), outbound_codec.clone()).await;
                } else {
                    info!("Anons kuyruğu boş.");
                }
            },

            else => {
                break;
            }
        }
    }
    
    // Graceful shutdown
    socket_reader_handle.abort();
    if let Some(session) = permanent_recording_session.lock().await.take() {
        warn!(uri = %session.output_uri, "Oturum kapanırken tamamlanmamış bir kayıt bulundu, sonlandırılıyor.");
        task::spawn(finalize_and_save_recording(session, config.app_state.clone()));
    }
    config.app_state.port_manager.remove_session(config.port).await;
    config.app_state.port_manager.quarantine_port(config.port).await;
    gauge!(ACTIVE_SESSIONS).decrement(1.0);
    info!("RTP oturumu başarıyla temizlendi ve kapatıldı.");
}


async fn start_playback(
    job: PlaybackJob, config: &RtpSessionConfig, socket: Arc<tokio::net::UdpSocket>,
    playback_finished_tx: mpsc::Sender<()>,
    permanent_recording_session: Arc<Mutex<Option<RecordingSession>>>,
    outbound_codec: Arc<Mutex<Option<AudioCodec>>>
) {

    let codec_to_use = outbound_codec.lock().await.unwrap_or(AudioCodec::Pcmu);
    
    match load_and_resample_samples_from_uri(&job.audio_uri, &config.app_state, &config.app_config).await {
        Ok(samples_16khz) => {
            if let Some(session) = &mut *permanent_recording_session.lock().await {
                session.outbound_samples.extend_from_slice(&samples_16khz);
            }

            task::spawn(async move {
                let _ = send_rtp_stream(&socket, job.target_addr, &samples_16khz, job.cancellation_token, codec_to_use).await;
                if let Err(e) = playback_finished_tx.send(()).await {
                     error!(error = %e, "Playback finished sinyali gönderilemedi.");
                }
            });
        },
        Err(e) => {
            error!(uri = %job.audio_uri, error = %e, "Anons dosyası yüklenemedi veya işlenemedi.");
            if let Err(e) = playback_finished_tx.send(()).await {
                error!(error = %e, "Hata durumu için playback finished sinyali gönderilemedi.");
            }
        }
    };
}