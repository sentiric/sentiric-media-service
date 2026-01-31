// sentiric-media-service/src/rtp/session.rs

use crate::config::AppConfig;
use crate::metrics::ACTIVE_SESSIONS;
use crate::rtp::codecs::{self, AudioCodec};
use crate::rtp::command::{AudioFrame, RecordingSession, RtpCommand};
use crate::rtp::session_utils::{finalize_and_save_recording, load_and_resample_samples_from_uri};
use crate::state::AppState;

use anyhow::Result;
use metrics::gauge;
use rand::Rng;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::{self, spawn_blocking};
use tokio::time::{self, Duration, Instant, MissedTickBehavior}; 
use tokio_util::sync::CancellationToken;

use tracing::{error, info, instrument, warn}; 

use sentiric_rtp_core::{CodecFactory, CodecType, RtpHeader, RtpPacket, RtcpPacket, RtpEndpoint};

const INACTIVITY_CHECK_INTERVAL: Duration = Duration::from_secs(5);
const MAX_SILENCE_PACKETS: usize = 50; 
const RTCP_INTERVAL: Duration = Duration::from_secs(5);

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

#[instrument(skip_all, fields(rtp_port = config.port))]
pub async fn rtp_session_handler(
    socket: Arc<tokio::net::UdpSocket>,
    mut command_rx: mpsc::Receiver<RtpCommand>,
    config: RtpSessionConfig,
) {
    info!("🎧 RTP Oturumu Başlatıldı (Port: {}). Dinleniyor...", config.port);

    let live_stream_sender = Arc::new(Mutex::new(None));
    let permanent_recording_session = Arc::new(Mutex::new(None));
    
    let endpoint = RtpEndpoint::new(None); 
    let mut pre_latch_target: Option<SocketAddr> = None;

    let current_codec_type = Arc::new(Mutex::new(CodecType::PCMU));
    
    let outbound_resampler = Arc::new(Mutex::new(
        codecs::StatefulResampler::new(24000, 8000).expect("Outbound Resampler Hatası"),
    ));
    
    let outbound_frame_size = outbound_resampler.lock().await.input_frame_size;

    let mut encoder = CodecFactory::create_encoder(CodecType::PCMU);
    let mut outbound_stream_rx: Option<mpsc::Receiver<Vec<u8>>> = None;
    
    let mut audio_accumulator: Vec<f32> = Vec::with_capacity(4096); 

    let rtp_ssrc: u32 = rand::thread_rng().gen();
    let mut rtp_seq: u16 = rand::thread_rng().gen();
    let mut rtp_ts: u32 = rand::thread_rng().gen();

    let mut playback_queue: VecDeque<PlaybackJob> = VecDeque::new();
    let mut is_playing_file = false;
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
                        if len < 4 || buf[..len].iter().all(|&b| b == b'\r' || b == b'\n' || b == 0) {
                            continue;
                        }
                        if rtp_packet_tx.send((buf[..len].to_vec(), remote_addr)).await.is_err() { break; }
                    }
                    Err(_) => break,
                }
            }
        })
    };

    let mut pacer = time::interval(Duration::from_millis(20));
    pacer.set_missed_tick_behavior(MissedTickBehavior::Skip);

    let mut rtcp_ticker = time::interval(RTCP_INTERVAL);
    rtcp_ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    let mut inactivity_checker = time::interval(INACTIVITY_CHECK_INTERVAL);
    inactivity_checker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut last_activity = Instant::now();
    let inactivity_limit = config.app_config.rtp_session_inactivity_timeout;

    let mut is_streaming_active = false;
    let mut silence_counter = 0;

    loop {
        tokio::select! {
            // A. PACER TICK (RTP Sending)
            _ = pacer.tick() => {
                if is_streaming_active {
                    let target = endpoint.get_target().or(pre_latch_target);
                    
                    if let Some(target_addr) = target {
                         if target_addr.ip().is_unspecified() || target_addr.port() == 0 {
                             // Hedef yoksa gönderme
                         } else {
                            if let Some(rx) = &mut outbound_stream_rx {
                                while let Ok(chunk) = rx.try_recv() {
                                    let samples_f32: Vec<f32> = chunk.chunks_exact(2)
                                        .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
                                        .collect();
                                    audio_accumulator.extend(samples_f32);
                                }
                            }

                            if audio_accumulator.len() >= outbound_frame_size {
                                silence_counter = 0;
                                let frame_in: Vec<f32> = audio_accumulator.drain(0..outbound_frame_size).collect();
                                
                                let resampler_clone = outbound_resampler.clone();
                                let samples_8k_result = spawn_blocking(move || {
                                    let mut guard = resampler_clone.blocking_lock();
                                    guard.process(&frame_in)
                                }).await.unwrap_or_else(|_| Ok(vec![0.0; 160]));

                                if let Ok(samples_8k_f32) = samples_8k_result {
                                    let samples_8k_i16: Vec<i16> = samples_8k_f32.into_iter()
                                        .map(|s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16)
                                        .collect();
                                    
                                    let encoded = encoder.encode(&samples_8k_i16);
                                    let current_type = encoder.get_type();
                                    let payload_size = if current_type == CodecType::G729 { 20 } else { 160 };
                                    let pt = match current_type { CodecType::PCMU => 0, CodecType::PCMA => 8, CodecType::G729 => 18, _ => 0 };

                                    for frame in encoded.chunks(payload_size) {
                                         let header = RtpHeader::new(pt, rtp_seq, rtp_ts, rtp_ssrc);
                                         let packet = RtpPacket { header, payload: frame.to_vec() };
                                         let _ = socket.send_to(&packet.to_bytes(), target_addr).await;
                                         rtp_seq = rtp_seq.wrapping_add(1);
                                         rtp_ts = rtp_ts.wrapping_add(160);
                                    }
                                }
                            } else if silence_counter < MAX_SILENCE_PACKETS {
                                 let silence_pcm = vec![0i16; 160];
                                 let encoded = encoder.encode(&silence_pcm);
                                 let current_type = encoder.get_type();
                                 let pt = match current_type { CodecType::PCMU => 0, CodecType::PCMA => 8, CodecType::G729 => 18, _ => 0 };
                                 let header = RtpHeader::new(pt, rtp_seq, rtp_ts, rtp_ssrc);
                                 let packet = RtpPacket { header, payload: encoded };
                                 let _ = socket.send_to(&packet.to_bytes(), target_addr).await;
                                 rtp_seq = rtp_seq.wrapping_add(1);
                                 rtp_ts = rtp_ts.wrapping_add(160);
                                 silence_counter += 1;
                            }
                         }
                    }
                }
            },

            // B. RTCP TICK (Keep-alive)
            _ = rtcp_ticker.tick() => {
                let target = endpoint.get_target().or(pre_latch_target);
                if let Some(mut target_addr) = target {
                    if !target_addr.ip().is_unspecified() && target_addr.port() != 0 {
                        let rtcp_port = target_addr.port() + 1;
                        target_addr.set_port(rtcp_port);
                        
                        let rtcp = RtcpPacket::new_sender_report(rtp_ssrc);
                        if let Err(e) = socket.send_to(&rtcp.to_bytes(), target_addr).await {
                            tracing::debug!("RTCP Send Error: {}", e);
                        }
                    }
                }
            },

            // C. COMMANDS
            Some(command) = command_rx.recv() => {
                last_activity = Instant::now();
                match command {
                    RtpCommand::PlayAudioUri { audio_uri, candidate_target_addr, cancellation_token, responder } => {
                        if endpoint.get_target().is_none() {
                             info!("🎯 [RTP] Pre-Latch Target Set -> {}", candidate_target_addr);
                             pre_latch_target = Some(candidate_target_addr);
                        }

                        let target_to_punch = endpoint.get_target().unwrap_or(candidate_target_addr);
                        let socket_clone = socket.clone();
                        tokio::spawn(async move {
                            let silence = vec![0x80u8; 160]; 
                            for _ in 0..20 { 
                                let _ = socket_clone.send_to(&silence, target_to_punch).await;
                                tokio::time::sleep(Duration::from_millis(20)).await;
                            }
                        });
                        
                        let target = endpoint.get_target().or(pre_latch_target).unwrap_or(candidate_target_addr);
                        let job = PlaybackJob { audio_uri, target_addr: target, cancellation_token, responder };
                        if !is_playing_file {
                            is_playing_file = true;
                            start_playback(job, &config, socket.clone(), playback_finished_tx.clone(), permanent_recording_session.clone()).await;
                        } else { 
                            playback_queue.push_back(job); 
                        }
                    },
                    RtpCommand::StartOutboundStream { audio_rx } => { 
                        info!("🎙️ TTS Outbound Stream Başlatıldı (24kHz Input).");
                        outbound_stream_rx = Some(audio_rx);
                        audio_accumulator.clear(); 
                        is_streaming_active = true; 
                        silence_counter = 0;
                        pacer.reset(); 
                    },
                    RtpCommand::StopOutboundStream => { 
                        info!("🎙️ TTS Outbound Stream Durduruldu.");
                        outbound_stream_rx = None; 
                        is_streaming_active = false;
                    },
                    RtpCommand::StartLiveAudioStream { stream_sender, .. } => { *live_stream_sender.lock().await = Some(stream_sender); },
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
                        } else { let _ = responder.send(Err("Kayıt bulunamadı".to_string())); }
                    },
                    RtpCommand::Shutdown => { 
                        info!("🛑 Shutdown komutu alındı.");
                        break; 
                    },
                    RtpCommand::StopAudio => { playback_queue.clear(); },
                    RtpCommand::StopLiveAudioStream => { *live_stream_sender.lock().await = None; },
                }
            },
            
            // D. INBOUND RTP HANDLING (GÜNCELLENDİ: TÜM KODEKLERİ DESTEKLER)
            Some((packet_data, remote_addr)) = rtp_packet_rx.recv() => {
                last_activity = Instant::now();
                if endpoint.latch(remote_addr) {
                    info!("🔄 NAT Latching: Hedef Kilitlendi -> {}", remote_addr);
                    pre_latch_target = None;
                }

                if packet_data.len() > 12 {
                    let pt = packet_data[1] & 0x7F;
                    let payload = &packet_data[12..];
                    
                    // 1. Dinamik Kodek Tespiti
                    let detected_codec_opt = AudioCodec::from_rtp_payload_type(pt).ok();
                    
                    if let Some(codec) = detected_codec_opt {
                        let mut current_type_guard = current_codec_type.lock().await;
                        let core_codec_type = codec.to_core_type();

                        // Eğer operatör kodek değiştirdiyse (Switch), biz de uyum sağlayalım
                        if *current_type_guard != core_codec_type {
                            info!("🔄 Codec Switch Detected: {:?} -> {:?}", *current_type_guard, core_codec_type);
                            *current_type_guard = core_codec_type;
                            // Giden ses (TX) için encoder'ı da güncelle
                            encoder = CodecFactory::create_encoder(core_codec_type);
                        }

                        // 2. Çözme İşlemi (rtp/codecs.rs'deki yeni merkezi fonksiyonu kullanıyoruz)
                        if let Ok(samples_16k) = codecs::decode_rtp_to_lpcm16(payload, codec) {
                            
                            // A. Canlı Stream'e Gönder (STT için)
                            let mut sender_guard = live_stream_sender.lock().await;
                            if let Some(sender) = &*sender_guard {
                                if !sender.is_closed() {
                                    let mut bytes = Vec::with_capacity(samples_16k.len() * 2);
                                    for sample in &samples_16k {
                                        bytes.extend_from_slice(&sample.to_le_bytes());
                                    }
                                    let frame = AudioFrame { 
                                        data: bytes.into(), 
                                        media_type: "audio/L16;rate=16000".to_string() 
                                    };
                                    let _ = sender.try_send(Ok(frame));
                                } else {
                                    *sender_guard = None;
                                }
                            }

                            // B. Kayda Yaz
                            let mut rec_guard = permanent_recording_session.lock().await;
                            if let Some(session) = rec_guard.as_mut() {
                                session.mixed_samples_16khz.extend(samples_16k.iter()); 
                            }
                        }
                    }
                }
            },
            
            // E. INACTIVITY CHECK
            _ = inactivity_checker.tick() => {
                if last_activity.elapsed() > inactivity_limit {
                    warn!("⏳ RTP Oturumu zaman aşımına uğradı (İnaktivite: {:?}). Oturum sonlandırılıyor.", last_activity.elapsed());
                    break;
                }
            },

            // F. PLAYBACK FINISHED
            Some(_) = playback_finished_rx.recv() => {
                last_activity = Instant::now();
                is_playing_file = false;
                if let Some(next_job) = playback_queue.pop_front() {
                    is_playing_file = true;
                    start_playback(next_job, &config, socket.clone(), playback_finished_tx.clone(), permanent_recording_session.clone()).await;
                }
            },
        }
    }
    
    // Cleanup
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

async fn start_playback(
    job: PlaybackJob, 
    config: &RtpSessionConfig, 
    socket: Arc<tokio::net::UdpSocket>,
    playback_finished_tx: mpsc::Sender<()>,
    _permanent_recording_session: Arc<Mutex<Option<RecordingSession>>>,
) {
    let responder = job.responder;

    match load_and_resample_samples_from_uri(&job.audio_uri, &config.app_state, &config.app_config).await {
        Ok(samples_16khz) => {
            task::spawn(async move {
                let local_codec = AudioCodec::Pcmu; 
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