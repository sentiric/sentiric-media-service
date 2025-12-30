// sentiric-media-service/src/rtp/session.rs
use crate::config::AppConfig;
use crate::rtp::codecs::{self, AudioCodec, StatefulResampler};
use crate::rtp::command::{AudioFrame, RtpCommand};
use crate::rtp::stream::send_rtp_stream;
use crate::state::AppState;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::{self, spawn_blocking};
use tokio_util::sync::CancellationToken;
use anyhow::Result;
use rand::Rng; // RTP state için gerekli

use super::command::RecordingSession;
use crate::rtp::session_utils::{finalize_and_save_recording, load_and_resample_samples_from_uri};
use crate::metrics::ACTIVE_SESSIONS;
use crate::utils::extract_uri_scheme;
use metrics::gauge;
use tracing::{debug, error, info, instrument, warn};
use webrtc_util::marshal::{Unmarshal, Marshal}; // Marshal eklendi

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
    let mut packet_buf = &packet_data[..];
    let packet = rtp::packet::Packet::unmarshal(&mut packet_buf).ok()?;
    let codec = codecs::AudioCodec::from_rtp_payload_type(packet.header.payload_type).ok()?;
    
    let samples = codecs::decode_g711_to_lpcm16(&packet.payload, codec, resampler).ok()?;
    Some((samples, codec))
}

#[instrument(
    skip_all,
    fields(rtp_port = config.port)
)]
pub async fn rtp_session_handler(
    socket: Arc<tokio::net::UdpSocket>,
    mut command_rx: mpsc::Receiver<RtpCommand>,
    config: RtpSessionConfig,
) {
    info!("Yeni RTP oturumu dinleyicisi başlatıldı.");
    
    // --- State Variables ---
    let live_stream_sender = Arc::new(Mutex::new(None));
    let permanent_recording_session = Arc::new(Mutex::new(None));
    let actual_remote_addr = Arc::new(Mutex::new(None));
    let outbound_codec = Arc::new(Mutex::new(None));
    
    // Inbound Resampler (Gelen sesi 16kHz'e çevirmek için)
    let inbound_resampler = Arc::new(Mutex::new(
        StatefulResampler::new(8000, 16000).expect("Inbound Resampler oluşturulamadı")
    ));

    // --- Outbound Streaming State ---
    let mut outbound_stream_rx: Option<mpsc::Receiver<Vec<u8>>> = None;
    // Outbound RTP state (SSRC, SeqNum, Timestamp)
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
                        if rtp_packet_tx.send((buf[..len].to_vec(), remote_addr)).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        })
    };

    // --- MAIN EVENT LOOP ---
    loop {
        tokio::select! {
            // 1. Gelen Komutlar (gRPC -> RTP Session)
            Some(command) = command_rx.recv() => {
                match command {
                    RtpCommand::PlayAudioUri { audio_uri, candidate_target_addr, cancellation_token, responder } => {
                        let addr = actual_remote_addr.lock().await.unwrap_or(candidate_target_addr);
                        // Loglama
                        if audio_uri.starts_with("data:") {
                            let truncated = &audio_uri[..std::cmp::min(60, audio_uri.len())];
                            debug!(uri.preview = %truncated, "PlayAudio (data URI) komutu alındı.");
                        } else {
                            debug!(uri = %audio_uri, "PlayAudio (file URI) komutu alındı.");
                        }
                        
                        let job = PlaybackJob { audio_uri, target_addr: addr, cancellation_token, responder };
                        
                        if !is_playing_file {
                            is_playing_file = true;
                            start_playback(job, &config, socket.clone(), playback_finished_tx.clone(), permanent_recording_session.clone(), outbound_codec.clone()).await;
                        } else {
                            playback_queue.push_back(job);
                        }
                    },
                    
                    // YENİ: Streaming Outbound Audio Başlatma
                    RtpCommand::StartOutboundStream { audio_rx } => {
                        debug!("Dış kaynaklı ses akışı (Outbound Streaming) başlatıldı.");
                        outbound_stream_rx = Some(audio_rx);
                    },
                    
                    // YENİ: Streaming Durdurma
                    RtpCommand::StopOutboundStream => {
                        debug!("Dış kaynaklı ses akışı durduruluyor.");
                        outbound_stream_rx = None;
                    },

                    RtpCommand::StartLiveAudioStream { stream_sender, .. } => {
                        debug!("Canlı ses akışı (STT - Inbound) başlatılıyor.");
                        *live_stream_sender.lock().await = Some(stream_sender);
                    },
                    
                    RtpCommand::StartPermanentRecording(mut session) => {
                        info!(uri = %session.output_uri, "Kalıcı kayıt oturumu başlatılıyor.");
                        session.mixed_samples_16khz.reserve(16000 * 60 * 5); 
                        *permanent_recording_session.lock().await = Some(session);
                    },
                    
                    RtpCommand::StopPermanentRecording { responder } => {
                        if let Some(session) = permanent_recording_session.lock().await.take() {
                            info!(uri = %session.output_uri, "Kalıcı kayıt durduruluyor.");
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
                    
                    RtpCommand::StopAudio => { 
                        playback_queue.clear();
                    },
                    
                    RtpCommand::StopLiveAudioStream => { 
                        *live_stream_sender.lock().await = None;
                    },
                }
            },
            
            // 2. Gelen RTP Paketleri (Kullanıcı -> RTP Session)
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
                    // Outbound için kullanılacak codec'i inbound'dan öğren
                    if outbound_codec.lock().await.is_none() { *outbound_codec.lock().await = Some(codec); }
                    
                    // Kayıt için mixle
                    if let Some(session) = &mut *permanent_recording_session.lock().await {
                        mix_audio(&mut session.mixed_samples_16khz, &samples_16khz);
                    }
                    
                    // Canlı dinleyiciye (STT) gönder
                    let mut sender_guard = live_stream_sender.lock().await;
                    if let Some(sender) = &*sender_guard {
                        if !sender.is_closed() {
                            let mut bytes = Vec::with_capacity(samples_16khz.len() * 2);
                            for &sample in &samples_16khz { bytes.extend_from_slice(&sample.to_le_bytes()); }
                            let frame = AudioFrame { data: bytes.into(), media_type: "audio/L16;rate=16000".to_string() };
                            if sender.try_send(Ok(frame)).is_err() { /* Buffer dolu, drop et */ }
                        } else {
                            *sender_guard = None;
                        }
                    }
                }
            },
            
            // 3. Dosya Çalma Bitiş Sinyali
            Some(_) = playback_finished_rx.recv() => {
                is_playing_file = false;
                debug!("Dosya oynatma tamamlandı.");
                if let Some(next_job) = playback_queue.pop_front() {
                    is_playing_file = true;
                    start_playback(next_job, &config, socket.clone(), playback_finished_tx.clone(), permanent_recording_session.clone(), outbound_codec.clone()).await;
                }
            },

            // 4. YENİ: Streaming Outbound Audio İşleme (gRPC Stream -> RTP Out)
            // Bu blok, outbound_stream_rx Some ise ve veri varsa çalışır.
            Some(chunk) = async { 
                if let Some(rx) = &mut outbound_stream_rx { rx.recv().await } else { std::future::pending().await } 
            } => {
                // Hedef adres belli mi?
                let target = *actual_remote_addr.lock().await;
                if let Some(target_addr) = target {
                    // Chunk, 16kHz PCM (S16LE) varsayılıyor (TelephonyActionService'den gelen standart)
                    // Bytes -> i16 Vec dönüşümü
                    let samples_16k: Vec<i16> = chunk.chunks_exact(2)
                        .map(|b| i16::from_le_bytes([b[0], b[1]]))
                        .collect();

                    if !samples_16k.is_empty() {
                        // 1. Encode (LPCM16 -> G711 PCMU/A)
                        let codec = outbound_codec.lock().await.unwrap_or(AudioCodec::Pcmu);
                        // Encode fonksiyonu şimdilik stateless (her chunk bağımsız)
                        if let Ok(encoded_payload) = codecs::encode_lpcm16_to_g711(&samples_16k, codec) {
                            
                            // 2. RTP Packet Hazırla
                            let payload_type = match codec { AudioCodec::Pcmu => 0, AudioCodec::Pcma => 8 };
                            
                            // Chunk'ları 20ms'lik paketlere böl (160 byte for G711)
                            for frame in encoded_payload.chunks(160) {
                                let packet = rtp::packet::Packet {
                                    header: rtp::header::Header {
                                        version: 2,
                                        payload_type,
                                        sequence_number: rtp_seq,
                                        timestamp: rtp_ts,
                                        ssrc: rtp_ssrc,
                                        ..Default::default()
                                    },
                                    payload: frame.to_vec().into(),
                                };

                                // 3. Gönder
                                if let Ok(raw_packet) = packet.marshal() {
                                    let _ = socket.send_to(&raw_packet, target_addr).await;
                                }

                                // 4. State Güncelle
                                rtp_seq = rtp_seq.wrapping_add(1);
                                rtp_ts = rtp_ts.wrapping_add(160);
                            }

                            // 5. Kayıt İçin Mixle (Bot'un sesi de kayda girmeli)
                            if let Some(session) = &mut *permanent_recording_session.lock().await {
                                mix_audio(&mut session.mixed_samples_16khz, &samples_16k);
                            }
                        }
                    }
                }
            },

            else => { break; } // Tüm kanallar kapandıysa çık
        }
    }
    
    socket_reader_handle.abort();
    if let Some(session) = permanent_recording_session.lock().await.take() {
        warn!(uri = %session.output_uri, "Oturum kapanırken tamamlanmamış kayıt bulundu.");
        task::spawn(finalize_and_save_recording(session, config.app_state.clone()));
    }
    config.app_state.port_manager.remove_session(config.port).await;
    config.app_state.port_manager.quarantine_port(config.port).await;
    gauge!(ACTIVE_SESSIONS).decrement(1.0);
    info!("RTP oturumu kapatıldı.");
}

// Yardımcı: Ses birleştirme (Mixing) - Saturating Add
fn mix_audio(buffer: &mut Vec<i16>, new_samples: &[i16]) {
    let current_len = buffer.len();
    for (i, sample) in new_samples.iter().enumerate() {
        if current_len + i < buffer.len() {
            buffer[current_len + i] = buffer[current_len + i].saturating_add(*sample);
        } else {
            buffer.push(*sample);
        }
    }
}

// Dosya Oynatma Yardımcısı (Eski mantık korundu)
#[instrument(skip_all, fields(target = %job.target_addr, uri.scheme = %extract_uri_scheme(&job.audio_uri)))]
async fn start_playback(
    job: PlaybackJob, config: &RtpSessionConfig, socket: Arc<tokio::net::UdpSocket>,
    playback_finished_tx: mpsc::Sender<()>,
    permanent_recording_session: Arc<Mutex<Option<RecordingSession>>>,
    outbound_codec: Arc<Mutex<Option<AudioCodec>>>
) {
    let codec_to_use = outbound_codec.lock().await.unwrap_or(AudioCodec::Pcmu);
    let responder = job.responder;

    match load_and_resample_samples_from_uri(&job.audio_uri, &config.app_state, &config.app_config).await {
        Ok(samples_16khz) => {
            if let Some(session) = &mut *permanent_recording_session.lock().await {
                mix_audio(&mut session.mixed_samples_16khz, &samples_16khz);
            }
            task::spawn(async move {
                let stream_result = send_rtp_stream(&socket, job.target_addr, &samples_16khz, job.cancellation_token, codec_to_use).await;
                if let Some(tx) = responder { let _ = tx.send(stream_result.map_err(anyhow::Error::from)); }
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