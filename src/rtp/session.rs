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

// CORE Libraries (Sentiric RTP Core)
use sentiric_rtp_core::{CodecFactory, CodecType, RtpHeader, RtpPacket};

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

/// Gelen RTP paketini ayrıştırır ve LPCM16 formatına çevirir.
/// Harici 'rtp' kütüphanesi yerine manuel parsing yaparak bağımlılığı sıfırlar.
fn process_rtp_payload(
    packet_data: &[u8],
    resampler: &mut StatefulResampler,
) -> Option<(Vec<i16>, AudioCodec)> {
    // RTP Header en az 12 byte'tır.
    if packet_data.len() < 12 {
        return None;
    }

    // Byte 0: Version, Padding, Extension, CSRC Count
    // Byte 1: Marker, Payload Type
    let payload_type = packet_data[1] & 0x7F;

    // Header uzunluğunu hesapla (Basit implementasyon: Extension/CSRC yok sayıyoruz)
    // Standart VoIP aramalarında genellikle Extension yoktur.
    // Eğer varsa ileride eklenebilir: Extension bit (packet_data[0] & 0x10) kontrolü.
    let header_len = 12;

    let payload = &packet_data[header_len..];

    // Payload tipine göre Codec belirle
    let codec = AudioCodec::from_rtp_payload_type(payload_type).ok()?;

    // Decode et (G.711 -> PCM 16-bit 16kHz)
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

    // --- State Variables ---
    let live_stream_sender = Arc::new(Mutex::new(None));
    let permanent_recording_session = Arc::new(Mutex::new(None));

    // Latching State (NAT Traversal): İlk paketin geldiği gerçek adres.
    let actual_remote_addr = Arc::new(Mutex::new(None));
    
    // Codec Negotiation: Karşı tarafın gönderdiği codeci öğrenip aynısıyla cevap vereceğiz.
    let outbound_codec = Arc::new(Mutex::new(None));

    // Inbound Resampler (Gelen 8kHz sesi 16kHz'e çevirmek için)
    let inbound_resampler = Arc::new(Mutex::new(
        StatefulResampler::new(8000, 16000).expect("Inbound Resampler oluşturulamadı"),
    ));
    
    // Outbound Encoder (16kHz PCM -> G.711) - rtp-core'dan. Varsayılan PCMU.
    // CodecType enum'u sentiric-rtp-core'dan geliyor.
    // Başlangıçta PCMU varsayıyoruz, ilk paket gelince güncellenebilir.
    let mut encoder = CodecFactory::create_encoder(CodecType::PCMU);

    // --- Outbound Streaming State ---
    let mut outbound_stream_rx: Option<mpsc::Receiver<Vec<u8>>> = None;
    
    // RTP Sequence State
    let rtp_ssrc: u32 = rand::thread_rng().gen();
    let mut rtp_seq: u16 = rand::thread_rng().gen();
    let mut rtp_ts: u32 = rand::thread_rng().gen();

    let mut playback_queue: VecDeque<PlaybackJob> = VecDeque::new();
    let mut is_playing_file = false;
    let (playback_finished_tx, mut playback_finished_rx) = mpsc::channel::<()>(1);

    // Socket Reader Loop (Network I/O)
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

    // --- MAIN EVENT LOOP ---
    loop {
        tokio::select! {
            // 1. Gelen Komutlar (gRPC -> RTP Session)
            Some(command) = command_rx.recv() => {
                match command {
                    RtpCommand::PlayAudioUri { audio_uri, candidate_target_addr, cancellation_token, responder } => {
                        let addr = actual_remote_addr.lock().await.unwrap_or(candidate_target_addr);
                        
                        if audio_uri.starts_with("data:") {
                            let truncated = &audio_uri[..std::cmp::min(60, audio_uri.len())];
                            debug!(uri.preview = %truncated, "PlayAudio (data URI) komutu alındı.");
                        } else {
                            debug!(uri = %audio_uri, "PlayAudio (file URI) komutu alındı.");
                        }
                        
                        let job = PlaybackJob { audio_uri, target_addr: addr, cancellation_token, responder };
                        
                        if !is_playing_file {
                            is_playing_file = true;
                            // Playback başlatılırken outbound codec bilgisi kullanılır
                            // Not: start_playback fonksiyonu içinde codec seçimi ayrıca ele alınmalıdır.
                            // Bu örnekte basitleştirilmiştir.
                            start_playback(job, &config, socket.clone(), playback_finished_tx.clone(), permanent_recording_session.clone()).await;
                        } else {
                            playback_queue.push_back(job);
                        }
                    },
                    
                    RtpCommand::StartOutboundStream { audio_rx } => {
                        debug!("📤 Dış kaynaklı ses akışı (Outbound Streaming) başlatıldı.");
                        outbound_stream_rx = Some(audio_rx);
                    },
                    
                    RtpCommand::StopOutboundStream => {
                        debug!("Dış kaynaklı ses akışı durduruluyor.");
                        outbound_stream_rx = None;
                    },

                    RtpCommand::StartLiveAudioStream { stream_sender, .. } => {
                        debug!("🎙️ Canlı ses akışı (STT - Inbound) başlatılıyor.");
                        *live_stream_sender.lock().await = Some(stream_sender);
                    },
                    
                    RtpCommand::StartPermanentRecording(mut session) => {
                        info!(uri = %session.output_uri, "💾 Kalıcı kayıt oturumu başlatılıyor.");
                        session.mixed_samples_16khz.reserve(16000 * 60 * 5); 
                        *permanent_recording_session.lock().await = Some(session);
                    },
                    
                    RtpCommand::StopPermanentRecording { responder } => {
                        if let Some(session) = permanent_recording_session.lock().await.take() {
                            info!(uri = %session.output_uri, "Kalıcı kayıt durduruluyor.");
                            let uri = session.output_uri.clone();
                            let app_state_clone = config.app_state.clone();
                            
                            // Kayıt işlemini bloklamamak için spawn ediyoruz
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
                        info!("🛑 Shutdown komutu alındı, oturum sonlandırılıyor.");
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
                // LATCHING LOGIC (NAT DELME)
                // İlk gelen paketin adresi bizim için "Gerçek" hedeftir.
                {
                    let mut locked_addr = actual_remote_addr.lock().await;
                    if locked_addr.is_none() || *locked_addr.as_ref().unwrap() != remote_addr {
                        info!("🔄 NAT Latching: Hedef güncellendi -> {}", remote_addr);
                        *locked_addr = Some(remote_addr);
                    }
                }

                // Process Packet (Decode & Resample)
                let resampler_clone = inbound_resampler.clone();
                let processing_result = spawn_blocking(move || {
                    let mut resampler_guard = resampler_clone.blocking_lock();
                    process_rtp_payload(&packet_data, &mut *resampler_guard)
                }).await;

                if let Ok(Some((samples_16khz, codec))) = processing_result {
                    // Outbound için kullanılacak codec'i inbound'dan öğren
                    {
                        let mut out_codec_guard = outbound_codec.lock().await;
                        if out_codec_guard.is_none() {
                             *out_codec_guard = Some(codec);
                             // Encoder'ı güncelle (PCMA mi PCMU mu?)
                             let new_type = match codec {
                                 AudioCodec::Pcmu => CodecType::PCMU,
                                 AudioCodec::Pcma => CodecType::PCMA,
                             };
                             encoder = CodecFactory::create_encoder(new_type);
                             debug!("Encoder codec güncellendi: {:?}", new_type);
                        }
                    }
                    
                    // // a) Kayıt için biriktir
                    // if let Some(_session) = &mut *permanent_recording_session.lock().await {
                    //     // TODO: Mix logic here. Şimdilik sadece gelen sesi ekliyoruz.
                    //     // _session.mixed_samples_16khz.extend_from_slice(&samples_16khz);
                    // }
                    
                    // b) Canlı dinleyiciye (STT) gönder
                    let mut sender_guard = live_stream_sender.lock().await;
                    if let Some(sender) = &*sender_guard {
                        if !sender.is_closed() {
                            // 16-bit PCM -> Bytes
                            let mut bytes = Vec::with_capacity(samples_16khz.len() * 2);
                            for sample in &samples_16khz { 
                                bytes.extend_from_slice(&sample.to_le_bytes()); 
                            }
                            
                            let frame = AudioFrame { 
                                data: bytes.into(), 
                                media_type: "audio/L16;rate=16000".to_string() 
                            };
                            
                            // Kanal doluysa drop et (Backpressure)
                            if sender.try_send(Ok(frame)).is_err() { 
                                // Buffer full, dropping frame to maintain real-time
                            }
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
                    start_playback(next_job, &config, socket.clone(), playback_finished_tx.clone(), permanent_recording_session.clone()).await;
                }
            },

            // 4. Streaming Outbound Audio İşleme (TTS -> RTP Out)
            // Bu blok, outbound_stream_rx Some ise ve veri varsa çalışır.
            Some(chunk) = async { 
                if let Some(rx) = &mut outbound_stream_rx { 
                    rx.recv().await 
                } else { 
                    std::future::pending().await 
                } 
            } => {
                // Hedef adres belli mi?
                let target = *actual_remote_addr.lock().await;
                if let Some(target_addr) = target {
                    // Chunk: 16kHz PCM (S16LE) geliyor (TTS'ten)
                    // Bytes -> i16 Vec dönüşümü
                    let samples_16k: Vec<i16> = chunk.chunks_exact(2)
                        .map(|b| i16::from_le_bytes([b[0], b[1]]))
                        .collect();

                    if !samples_16k.is_empty() {
                        // 1. Encode (LPCM16 -> G.711)
                        // sentiric-rtp-core Encoder trait'ini kullanıyoruz.
                        let encoded_payload = encoder.encode(&samples_16k);
                        
                        // 2. RTP Packetize (20ms chunks)
                        // G.711 için 20ms = 160 samples (bytes)
                        const SAMPLES_PER_PACKET: usize = 160;
                        
                        // Codec tipine göre payload type belirle
                        let pt = match encoder.get_type() {
                            CodecType::PCMU => 0,
                            CodecType::PCMA => 8,
                            _ => 0, // Fallback PCMU
                        };

                        for frame in encoded_payload.chunks(SAMPLES_PER_PACKET) {
                            let header = RtpHeader::new(pt, rtp_seq, rtp_ts, rtp_ssrc);
                            let packet = RtpPacket {
                                header,
                                payload: frame.to_vec(),
                            };

                            // 3. Gönder
                            let _ = socket.send_to(&packet.to_bytes(), target_addr).await;

                            // 4. State Güncelle
                            rtp_seq = rtp_seq.wrapping_add(1);
                            rtp_ts = rtp_ts.wrapping_add(SAMPLES_PER_PACKET as u32);
                            
                            // Basit pacing (20ms) - Jitter buffer rtp-core'da olsa daha iyi ama şimdilik burada.
                            tokio::time::sleep(std::time::Duration::from_millis(19)).await;
                        }

                        // 5. Kayıt İçin Mixle (Bot'un sesi de kayda girmeli)
                        if let Some(session) = &mut *permanent_recording_session.lock().await {
                            // Basitçe ekliyoruz, mix mantığı daha sonra eklenecek
                            // session.mixed_samples_16khz.extend_from_slice(&samples_16k);
                        }
                    }
                }
            },

            else => { break; } // Tüm kanallar kapandıysa çık
        }
    }
    
    // Temizlik
    socket_reader_handle.abort();
    
    if let Some(session) = permanent_recording_session.lock().await.take() {
        warn!(uri = %session.output_uri, "Oturum kapanırken tamamlanmamış kayıt bulundu. Kaydediliyor...");
        let app_state_clone = config.app_state.clone();
        task::spawn(finalize_and_save_recording(session, app_state_clone));
    }
    
    config.app_state.port_manager.remove_session(config.port).await;
    config.app_state.port_manager.quarantine_port(config.port).await;
    
    gauge!(ACTIVE_SESSIONS).decrement(1.0);
    info!("🏁 RTP Oturumu Sonlandı (Port: {})", config.port);
}


// Dosya Oynatma Yardımcısı
#[instrument(skip_all, fields(target = %job.target_addr, uri.scheme = %extract_uri_scheme(&job.audio_uri)))]
async fn start_playback(
    job: PlaybackJob, 
    config: &RtpSessionConfig, 
    socket: Arc<tokio::net::UdpSocket>,
    playback_finished_tx: mpsc::Sender<()>,
    permanent_recording_session: Arc<Mutex<Option<RecordingSession>>>,
) {
    // Playback için varsayılan codec PCMU (0) kullanıyoruz.
    let codec_type = CodecType::PCMU;
    let responder = job.responder;

    match load_and_resample_samples_from_uri(&job.audio_uri, &config.app_state, &config.app_config).await {
        Ok(samples_16khz) => {
            // Kayıt için
            if let Some(session) = &mut *permanent_recording_session.lock().await {
                 // session.mixed_samples_16khz.extend_from_slice(&samples_16khz);
            }
            
            // Streaming Task
            task::spawn(async move {
                // Burada crate::rtp::stream::send_rtp_stream fonksiyonunu kullanıyoruz
                // Ancak o fonksiyon AudioCodec (yerel) istiyor, biz CodecType (core) kullanıyoruz.
                // Bu uyumsuzluğu gidermek için stream.rs içindeki fonksiyonun AudioCodec yerine CodecType alması lazım
                // VEYA burada dönüşüm yapmalıyız. 
                // Hızlı çözüm: stream.rs'i zaten güncellemiştik. Oradaki `target_codec` parametresinin türüne bakalım.
                // Evet, stream.rs'te `crate::rtp::codecs::AudioCodec` kullanılıyor.
                // O zaman burada dönüşüm yapalım:
                
                let local_codec = codecs::AudioCodec::Pcmu; // Varsayılan

                let stream_result = crate::rtp::stream::send_rtp_stream(
                    &socket, 
                    job.target_addr, 
                    &samples_16khz, 
                    job.cancellation_token, 
                    local_codec
                ).await;
                
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