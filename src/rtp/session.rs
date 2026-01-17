// sentiric-media-service/src/rtp/session.rs

use crate::config::AppConfig;
use crate::metrics::ACTIVE_SESSIONS;
use crate::rtp::codecs::{self, AudioCodec, StatefulResampler};
use crate::rtp::command::{AudioFrame, RecordingSession, RtpCommand};
use crate::rtp::session_utils::{finalize_and_save_recording, load_and_resample_samples_from_uri};
use crate::state::AppState;
use crate::utils::extract_uri_scheme;

use anyhow::Result;
use metrics::gauge;
use rand::Rng;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::{self, spawn_blocking};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, instrument, warn};

use sentiric_rtp_core::{CodecFactory, CodecType, RtpHeader, RtpPacket};

// ... (RtpSessionConfig ve PlaybackJob structları aynı) ...
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
    responder: Option<oneshot::Sender<Result<()>>>,
}

fn process_rtp_payload(
    packet_data: &[u8],
    resampler: &mut StatefulResampler,
) -> Option<(Vec<i16>, AudioCodec)> {
    if packet_data.len() < 12 { return None; }
    let payload_type = packet_data[1] & 0x7F;
    let header_len = 12;
    let payload = &packet_data[header_len..];
    let codec = AudioCodec::from_rtp_payload_type(payload_type).ok()?;
    let samples = codecs::decode_g711_to_lpcm16(payload, codec, resampler).ok()?;
    Some((samples, codec))
}

#[instrument(skip_all, fields(rtp_port = config.port))]
pub async fn rtp_session_handler(
    socket: Arc<tokio::net::UdpSocket>,
    mut command_rx: mpsc::Receiver<RtpCommand>,
    config: RtpSessionConfig,
) {
    info!("🎧 RTP Oturumu Başlatıldı (Port: {}). Dinleniyor...", config.port);

    let live_stream_sender = Arc::new(Mutex::new(None));
    let permanent_recording_session = Arc::new(Mutex::new(None));

    // Latching State
    let actual_remote_addr = Arc::new(Mutex::new(None));
    
    // YENİ: SDP'den gelen ilk hedef adresi sakla (Fallback için)
    let initial_target_addr = Arc::new(Mutex::new(None)); 

    let outbound_codec = Arc::new(Mutex::new(None));
    let inbound_resampler = Arc::new(Mutex::new(
        StatefulResampler::new(8000, 16000).expect("Inbound Resampler oluşturulamadı"),
    ));
    let mut encoder = CodecFactory::create_encoder(CodecType::PCMU);
    let mut outbound_stream_rx: Option<mpsc::Receiver<Vec<u8>>> = None;
    
    let rtp_ssrc: u32 = rand::thread_rng().gen();
    let mut rtp_seq: u16 = rand::thread_rng().gen();
    let mut rtp_ts: u32 = rand::thread_rng().gen();

    let mut playback_queue: VecDeque<PlaybackJob> = VecDeque::new();
    let mut is_playing_file = false;
    let (playback_finished_tx, mut playback_finished_rx) = mpsc::channel::<()>(1);

    // Socket Reader Loop
    let (rtp_packet_tx, mut rtp_packet_rx) = mpsc::channel::<(Vec<u8>, SocketAddr)>(256);
    let socket_reader_handle = {
        let socket = socket.clone();
        let rtp_packet_tx = rtp_packet_tx.clone();
        task::spawn(async move {
            let mut buf = [0u8; 2048];
            loop {
                match socket.recv_from(&mut buf).await {
                    Ok((len, remote_addr)) => {
                        if rtp_packet_tx
                            .send((buf[..len].to_vec(), remote_addr))
                            .await
                            .is_err()
                        {
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
                    RtpCommand::PlayAudioUri { audio_uri, candidate_target_addr, cancellation_token, responder } => {
                        // YENİ: İlk gelen hedefi sakla
                        {
                            let mut init_addr = initial_target_addr.lock().await;
                            if init_addr.is_none() {
                                *init_addr = Some(candidate_target_addr);
                                info!("🎯 Initial RTP Target set to: {}", candidate_target_addr);
                            }
                        }

                        let addr = actual_remote_addr.lock().await.unwrap_or(candidate_target_addr);
                        
                        info!(uri = %audio_uri, target = %addr, "▶️ PlayAudioUri isteği alındı. Oynatılıyor...");
                        
                        let job = PlaybackJob { audio_uri, target_addr: addr, cancellation_token, responder };
                        if !is_playing_file {
                            is_playing_file = true;
                            start_playback(job, &config, socket.clone(), playback_finished_tx.clone(), permanent_recording_session.clone()).await;
                        } else {
                            playback_queue.push_back(job);
                        }
                    },
                    
                    RtpCommand::StartOutboundStream { audio_rx } => {
                        info!("📤 Dış kaynaklı ses akışı (TTS) başlatıldı.");
                        outbound_stream_rx = Some(audio_rx);
                    },
                    
                    RtpCommand::StopOutboundStream => {
                        info!("Dış kaynaklı ses akışı durduruluyor.");
                        outbound_stream_rx = None;
                    },

                    RtpCommand::StartLiveAudioStream { stream_sender, .. } => {
                        info!("🎙️ Canlı ses akışı (STT) başlatılıyor.");
                        *live_stream_sender.lock().await = Some(stream_sender);
                    },
                    
                    RtpCommand::StartPermanentRecording(mut session) => {
                        info!("💾 Kalıcı kayıt oturumu başlatılıyor.");
                        session.mixed_samples_16khz.reserve(16000 * 60 * 5); 
                        *permanent_recording_session.lock().await = Some(session);
                    },
                    
                    RtpCommand::StopPermanentRecording { responder } => {
                        if let Some(session) = permanent_recording_session.lock().await.take() {
                            info!("Kalıcı kayıt durduruluyor.");
                            let uri = session.output_uri.clone();
                            let app_state_clone = config.app_state.clone();
                            tokio::spawn(async move {
                                let _ = finalize_and_save_recording(session, app_state_clone).await;
                            });
                            let _ = responder.send(Ok(uri));
                        } else {
                            warn!("Durdurulacak aktif bir kayıt bulunamadı.");
                            let _ = responder.send(Err("Kayıt bulunamadı".to_string()));
                        }
                    },
                    
                    RtpCommand::Shutdown => {
                        info!("🛑 Shutdown komutu alındı.");
                        break;
                    },
                    
                    RtpCommand::StopAudio => { playback_queue.clear(); },
                    
                    RtpCommand::StopLiveAudioStream => { *live_stream_sender.lock().await = None; },
                }
            },
            
            Some((packet_data, remote_addr)) = rtp_packet_rx.recv() => {
                {
                    let mut locked_addr = actual_remote_addr.lock().await;
                    if locked_addr.is_none() || *locked_addr.as_ref().unwrap() != remote_addr {
                        info!("🔄 NAT Latching: Hedef güncellendi -> {}", remote_addr);
                        *locked_addr = Some(remote_addr);
                    }
                }
                
                if rtp_seq % 100 == 0 {
                    debug!("📥 RTP Inbound: {} bytes from {}", packet_data.len(), remote_addr);
                }

                let resampler_clone = inbound_resampler.clone();
                let processing_result = spawn_blocking(move || {
                    let mut resampler_guard = resampler_clone.blocking_lock();
                    process_rtp_payload(&packet_data, &mut *resampler_guard)
                }).await;

                if let Ok(Some((samples_16khz, codec))) = processing_result {
                    {
                        let mut out_codec_guard = outbound_codec.lock().await;
                        if out_codec_guard.is_none() {
                             *out_codec_guard = Some(codec);
                             let new_type = match codec {
                                 AudioCodec::Pcmu => CodecType::PCMU,
                                 AudioCodec::Pcma => CodecType::PCMA,
                             };
                             encoder = CodecFactory::create_encoder(new_type);
                             info!("Encoder codec güncellendi: {:?}", new_type);
                        }
                    }
                    
                    let mut sender_guard = live_stream_sender.lock().await;
                    if let Some(sender) = &*sender_guard {
                        if !sender.is_closed() {
                             let mut bytes = Vec::with_capacity(samples_16khz.len() * 2);
                             for sample in &samples_16khz { bytes.extend_from_slice(&sample.to_le_bytes()); }
                             let frame = AudioFrame { data: bytes.into(), media_type: "audio/L16;rate=16000".to_string() };
                             let _ = sender.try_send(Ok(frame));
                        } else {
                            *sender_guard = None;
                        }
                    }
                }
            },
            
            Some(_) = playback_finished_rx.recv() => {
                is_playing_file = false;
                info!("✅ Dosya oynatma tamamlandı.");
                if let Some(next_job) = playback_queue.pop_front() {
                    is_playing_file = true;
                    start_playback(next_job, &config, socket.clone(), playback_finished_tx.clone(), permanent_recording_session.clone()).await;
                }
            },

            // --- DEĞİŞTİRİLEN KISIM: TTS Outbound Handling ---
            Some(chunk) = async { 
                if let Some(rx) = &mut outbound_stream_rx { 
                    rx.recv().await 
                } else { 
                    std::future::pending().await 
                } 
            } => {
                // HEDEF SEÇİMİ (FALLBACK MANTIĞI)
                let target = {
                    let actual = *actual_remote_addr.lock().await;
                    if actual.is_some() {
                        actual
                    } else {
                        *initial_target_addr.lock().await
                    }
                };

                if let Some(target_addr) = target {
                    let samples_16k: Vec<i16> = chunk.chunks_exact(2)
                        .map(|b| i16::from_le_bytes([b[0], b[1]]))
                        .collect();

                    if !samples_16k.is_empty() {
                        let encoded_payload = encoder.encode(&samples_16k);
                        const SAMPLES_PER_PACKET: usize = 160;
                        let pt = match encoder.get_type() {
                            CodecType::PCMU => 0, CodecType::PCMA => 8, _ => 0,
                        };

                        for frame in encoded_payload.chunks(SAMPLES_PER_PACKET) {
                            let header = RtpHeader::new(pt, rtp_seq, rtp_ts, rtp_ssrc);
                            let packet = RtpPacket { header, payload: frame.to_vec() };
                            
                            if rtp_seq % 50 == 0 {
                                info!("📤 RTP Outbound (TTS): Seq={} TS={} Size={} Target={}", rtp_seq, rtp_ts, frame.len(), target_addr);
                            }

                            if let Err(e) = socket.send_to(&packet.to_bytes(), target_addr).await {
                                error!(error = %e, "RTP gönderim hatası");
                            }

                            rtp_seq = rtp_seq.wrapping_add(1);
                            rtp_ts = rtp_ts.wrapping_add(SAMPLES_PER_PACKET as u32);
                            tokio::time::sleep(std::time::Duration::from_millis(19)).await;
                        }

                        if let Some(session) = &mut *permanent_recording_session.lock().await {
                            // Kayıt mix mantığı buraya
                        }
                    }
                } else {
                    // Hiçbir hedef yoksa paketleri at
                    if rtp_seq % 100 == 0 {
                        warn!("⚠️ RTP Outbound: Hedef adres HİÇ YOK (Ne Latching Ne SDP), paketler kayboluyor.");
                    }
                }
            },
            
            else => { break; }
        }
    }
    
    socket_reader_handle.abort();
    if let Some(session) = permanent_recording_session.lock().await.take() {
        let app_state_clone = config.app_state.clone();
        task::spawn(finalize_and_save_recording(session, app_state_clone));
    }
    config.app_state.port_manager.remove_session(config.port).await;
    config.app_state.port_manager.quarantine_port(config.port).await;
    gauge!(ACTIVE_SESSIONS).decrement(1.0);
    info!("🏁 RTP Oturumu Sonlandı (Port: {})", config.port);
}

// start_playback yardımcı fonksiyonu aynı kalıyor, sadece target_addr kullanımı zaten doğru.
#[instrument(skip_all, fields(target = %job.target_addr, uri.scheme = %extract_uri_scheme(&job.audio_uri)))]
async fn start_playback(
    job: PlaybackJob, 
    config: &RtpSessionConfig, 
    socket: Arc<tokio::net::UdpSocket>,
    playback_finished_tx: mpsc::Sender<()>,
    permanent_recording_session: Arc<Mutex<Option<RecordingSession>>>,
) {
    let responder = job.responder;

    match load_and_resample_samples_from_uri(&job.audio_uri, &config.app_state, &config.app_config).await {
        Ok(samples_16khz) => {
            if let Some(_session) = &mut *permanent_recording_session.lock().await {
                 // Kayıt
            }
            task::spawn(async move {
                let local_codec = codecs::AudioCodec::Pcmu; 
                info!(samples = samples_16khz.len(), target = %job.target_addr, "RTP dosya akışı başlatılıyor...");

                let stream_result = crate::rtp::stream::send_rtp_stream(
                    &socket, 
                    job.target_addr, 
                    &samples_16khz, 
                    job.cancellation_token, 
                    local_codec
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