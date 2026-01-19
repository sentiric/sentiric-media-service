// sentiric-media-service/src/rtp/session.rs

use crate::config::AppConfig;
use crate::metrics::ACTIVE_SESSIONS;
use crate::rtp::codecs::{StatefulResampler, AudioCodec, RESAMPLER_INPUT_FRAME_SIZE};
use crate::rtp::command::{RecordingSession, RtpCommand};
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
use tokio_util::sync::CancellationToken;
use tracing::{error, info, instrument, warn};

use sentiric_rtp_core::{CodecFactory, CodecType, RtpHeader, RtpPacket};

// Configuration
const DEBUG_BYPASS_RESAMPLER: bool = false;

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

    // State Variables
    let live_stream_sender = Arc::new(Mutex::new(None));
    let permanent_recording_session = Arc::new(Mutex::new(None));
    let actual_remote_addr = Arc::new(Mutex::new(None));
    let initial_target_addr = Arc::new(Mutex::new(None)); 

    // Codec State: Varsayılan PCMU, ama gelen pakete göre değişecek.
    let current_codec_type = Arc::new(Mutex::new(CodecType::PCMU));
    
    // Resamplers
    let _inbound_resampler = Arc::new(Mutex::new(
        StatefulResampler::new(8000, 16000).expect("Inbound Resampler Hatası"),
    ));

    // Outbound (16k -> 8k) - TTS'den kullanıcıya
    let outbound_resampler = Arc::new(Mutex::new(
        StatefulResampler::new(16000, 8000).expect("Outbound Resampler Hatası"),
    ));

    // Encoder Başlat (Varsayılan PCMU)
    let mut encoder = CodecFactory::create_encoder(CodecType::PCMU);
    
    // Outbound Stream Channels & Buffers
    let mut outbound_stream_rx: Option<mpsc::Receiver<Vec<u8>>> = None;
    
    // --- BUFFER ---
    // 320 sample = 20ms @ 16kHz
    let mut audio_accumulator: Vec<f32> = Vec::with_capacity(4096); 

    // RTP Sequencing
    let rtp_ssrc: u32 = rand::thread_rng().gen();
    let mut rtp_seq: u16 = rand::thread_rng().gen();
    let mut rtp_ts: u32 = rand::thread_rng().gen();

    // Playback Logic
    let mut playback_queue: VecDeque<PlaybackJob> = VecDeque::new();
    let mut is_playing_file = false;
    let (playback_finished_tx, mut playback_finished_rx) = mpsc::channel::<()>(1);

    // Socket Reader Task
    let (rtp_packet_tx, mut rtp_packet_rx) = mpsc::channel::<(Vec<u8>, SocketAddr)>(256);
    let socket_reader_handle = {
        let socket = socket.clone();
        let rtp_packet_tx = rtp_packet_tx.clone();
        task::spawn(async move {
            let mut buf = [0u8; 2048];
            loop {
                match socket.recv_from(&mut buf).await {
                    Ok((len, remote_addr)) => {
                        if rtp_packet_tx.send((buf[..len].to_vec(), remote_addr)).await.is_err() { break; }
                    }
                    Err(_) => break,
                }
            }
        })
    };

    loop {
        tokio::select! {
            // 1. COMMAND CHANNEL HANDLING
            Some(command) = command_rx.recv() => {
                match command {
                    RtpCommand::PlayAudioUri { audio_uri, candidate_target_addr, cancellation_token, responder } => {
                        {
                            let mut init_addr = initial_target_addr.lock().await;
                            if init_addr.is_none() {
                                *init_addr = Some(candidate_target_addr);
                                info!("🎯 Initial RTP Target set to: {}", candidate_target_addr);
                            }
                        }
                        let addr = actual_remote_addr.lock().await.unwrap_or(candidate_target_addr);
                        let job = PlaybackJob { audio_uri, target_addr: addr, cancellation_token, responder };
                        if !is_playing_file {
                            is_playing_file = true;
                            start_playback(job, &config, socket.clone(), playback_finished_tx.clone(), permanent_recording_session.clone()).await;
                        } else { playback_queue.push_back(job); }
                    },
                    RtpCommand::StartOutboundStream { audio_rx } => { 
                        info!("🎙️ TTS Outbound Stream Başlatıldı.");
                        outbound_stream_rx = Some(audio_rx);
                        audio_accumulator.clear(); 
                    },
                    RtpCommand::StopOutboundStream => { 
                        info!("🎙️ TTS Outbound Stream Durduruldu.");
                        outbound_stream_rx = None; 
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
                    RtpCommand::Shutdown => { break; },
                    RtpCommand::StopAudio => { playback_queue.clear(); },
                    RtpCommand::StopLiveAudioStream => { *live_stream_sender.lock().await = None; },
                }
            },
            
            // 2. INBOUND RTP HANDLING (Kullanıcıdan Gelen Ses & Codec Tespiti)
            Some((packet_data, remote_addr)) = rtp_packet_rx.recv() => {
                // NAT Latching
                {
                    let mut locked_addr = actual_remote_addr.lock().await;
                    if locked_addr.is_none() || *locked_addr.as_ref().unwrap() != remote_addr {
                        info!("🔄 NAT Latching: Hedef güncellendi -> {}", remote_addr);
                        *locked_addr = Some(remote_addr);
                    }
                }
                
                // Codec Detection & Switching Logic (G.729 Desteği)
                if packet_data.len() > 12 {
                    let pt = packet_data[1] & 0x7F;
                    
                    if let Some(new_codec) = CodecType::from_u8(pt) {
                        let mut current = current_codec_type.lock().await;
                        // Sadece PCMU/PCMA/G729/G722 değişiminde log bas
                        if *current != new_codec && (new_codec == CodecType::G729 || new_codec == CodecType::PCMU || new_codec == CodecType::PCMA) {
                            info!("🔄 Codec Switch Detected: {:?} -> {:?}", *current, new_codec);
                            *current = new_codec;
                            
                            // Encoder'ı anında güncelle (G.711 -> G.729 geçişi burada olur)
                            encoder = CodecFactory::create_encoder(new_codec);
                        }
                    }
                }
            },
            
            // 3. PLAYBACK FINISHED
            Some(_) = playback_finished_rx.recv() => {
                is_playing_file = false;
                if let Some(next_job) = playback_queue.pop_front() {
                    is_playing_file = true;
                    start_playback(next_job, &config, socket.clone(), playback_finished_tx.clone(), permanent_recording_session.clone()).await;
                }
            },

            // 4. OUTBOUND STREAM HANDLING (TTS -> RTP)
            Some(chunk) = async { if let Some(rx) = &mut outbound_stream_rx { rx.recv().await } else { std::future::pending().await } } => {
                
                let target = {
                    let actual = *actual_remote_addr.lock().await;
                    if actual.is_some() { actual } else { *initial_target_addr.lock().await }
                };

                if let Some(target_addr) = target {
                    // A. Byte Chunk -> f32 Samples
                    let samples_16k_f32: Vec<f32> = chunk
                        .chunks_exact(2)
                        .map(|b| {
                            let s16 = i16::from_le_bytes([b[0], b[1]]);
                            s16 as f32 / 32768.0 
                        })
                        .collect();

                    // B. Biriktiriciye Ekle
                    audio_accumulator.extend(samples_16k_f32);

                    // C. Yeterli Veri Oldukça İşle
                    while audio_accumulator.len() >= RESAMPLER_INPUT_FRAME_SIZE {
                        let frame_in: Vec<f32> = audio_accumulator.drain(0..RESAMPLER_INPUT_FRAME_SIZE).collect();

                        // 1. Resampling (16kHz -> 8kHz)
                        let samples_8k_result = if DEBUG_BYPASS_RESAMPLER {
                            Ok(frame_in.iter().step_by(2).cloned().collect())
                        } else {
                            let resampler_clone = outbound_resampler.clone();
                            spawn_blocking(move || {
                                let mut guard = resampler_clone.blocking_lock();
                                guard.process(&frame_in)
                            }).await.unwrap_or_else(|e| Err(anyhow::anyhow!("JoinError: {}", e)))
                        };

                        match samples_8k_result {
                            Ok(samples_8k_f32) => {
                                // 2. f32 -> i16 
                                let samples_8k_i16: Vec<i16> = samples_8k_f32.into_iter()
                                    .map(|s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16)
                                    .collect();

                                // 3. Encode (PCMU/PCMA/G729) - Encoder dinamik olarak güncellenmiştir
                                let encoded_payload = encoder.encode(&samples_8k_i16);
                                
                                // 4. Packetize & Send
                                // G.711: 160 bytes (20ms), G.729: 20 bytes (20ms)
                                let current_type = encoder.get_type();
                                
                                // Paket boyutunu codec tipine göre dinamik belirle
                                let payload_size = if current_type == CodecType::G729 { 
                                    // G.729 için genelde 2 frame (20ms) bir pakete sığar: 10 + 10 = 20 byte
                                    20 
                                } else { 
                                    // G.711 için 20ms = 160 byte
                                    160 
                                };
                                
                                let pt = match current_type {
                                    CodecType::PCMU => 0,
                                    CodecType::PCMA => 8,
                                    CodecType::G729 => 18,
                                    _ => 0
                                };

                                for frame in encoded_payload.chunks(payload_size) {
                                    let header = RtpHeader::new(pt, rtp_seq, rtp_ts, rtp_ssrc);
                                    let packet = RtpPacket { header, payload: frame.to_vec() };
                                    
                                    if let Err(e) = socket.send_to(&packet.to_bytes(), target_addr).await {
                                        warn!("RTP Send Error: {}", e);
                                    }

                                    rtp_seq = rtp_seq.wrapping_add(1);
                                    rtp_ts = rtp_ts.wrapping_add(160); 
                                }
                            },
                            Err(e) => {
                                error!("❌ Resampling Error: {}", e);
                                audio_accumulator.clear(); 
                            }
                        }
                    }
                } else {
                    audio_accumulator.clear();
                    warn!("⚠️ RTP Hedefi bilinmiyor (No Latching). Paket düşürüldü.");
                }
            },
            
            else => { break; }
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

// Dosya Oynatma Fonksiyonu
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
            // ... (Kayıt logic - Şimdilik devre dışı/pasif) ...
            task::spawn(async move {
                // Burada dosya oynatırken varsayılan olarak PCMU kullanıyoruz (basitlik için).
                // İdealde buranın da 'current_codec_type' kullanması gerekirdi ama playback
                // ayrı bir issue. Şu anki odak noktamız TTS akışı.
                // Kod derlensin diye AudioCodec::Pcmu doğrudan kullanıldı.
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