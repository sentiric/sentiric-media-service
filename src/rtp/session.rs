// File: src/rtp/session.rs (TAM VE NİHAİ DÜZELTİLMİŞ HALİ)
use crate::audio::load_or_get_from_cache;
use crate::config::AppConfig;
use crate::metrics::ACTIVE_SESSIONS;
use crate::rabbitmq;
use crate::rtp::codecs::{self, AudioCodec};
use crate::rtp::command::{AudioFrame, RtpCommand};
use crate::rtp::stream::{decode_audio_with_symphonia, send_rtp_stream};
use crate::rtp::writers;
use crate::state::AppState;
use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose, Engine};
use hound::WavWriter;
use lapin::{options::BasicPublishOptions, BasicProperties};
use metrics::gauge;
use rtp::packet::Packet;
use std::io::Cursor;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::task::{spawn_blocking, JoinHandle};
use tokio::time::{sleep, Instant};
use tokio_util::sync::CancellationToken;
use tonic::Status;
use tracing::{debug, error, info, instrument, warn};
use webrtc_util::marshal::Unmarshal;
use rubato::{Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};

use super::command::RecordingSession;

pub struct RtpSessionConfig {
    pub app_state: AppState,
    pub app_config: Arc<AppConfig>,
    pub port: u16,
}

#[derive(Debug)]
struct ProcessedAudio {
    samples_16khz: Vec<i16>,
    source_codec: AudioCodec,
}

// =========================================================================
// === SES YÜKLEME VE İŞLEME DÜZELTMESİ: Anonsları 16kHz'e yükseltme ===
// =========================================================================

async fn load_and_resample_samples_from_uri(
    uri: &str,
    app_state: &AppState,
    config: &Arc<AppConfig>,
) -> Result<Arc<Vec<i16>>> {
    // 1. Data URI'lerini (TTS'den gelen) doğrudan 16kHz LPCM olarak decode et
    if uri.starts_with("data:") {
        info!("Data URI'sinden (TTS) ses yükleniyor...");
        let (_media_type, base64_data) = uri
            .strip_prefix("data:")
            .and_then(|s| s.split_once(";base64,"))
            .context("Geçersiz data URI formatı")?;
        let audio_bytes = general_purpose::STANDARD
            .decode(base64_data)
            .context("Base64 verisi çözümlenemedi")?;
        // Symphonia, sesi doğrudan sistemin standart 16kHz formatına çevirir.
        return Ok(Arc::new(decode_audio_with_symphonia(audio_bytes)?));
    }

    // 2. File URI'lerini (anonslar) önce 8kHz olarak yükle, sonra 16kHz'e yükselt
    if let Some(path_part) = uri.strip_prefix("file://") {
        let mut final_path = PathBuf::from(&config.assets_base_path);
        final_path.push(path_part.trim_start_matches('/'));

        // Anons dosyasını 8kHz olarak yükle (WAV dosyaları bu formatta olmalı)
        let samples_from_file_8khz = load_or_get_from_cache(&app_state.audio_cache, &final_path).await?;
        
        info!(samples_count = samples_from_file_8khz.len(), "Anons (8kHz) yüklendi, 16kHz'e yükseltiliyor...");
        
        // Yeniden örnekleme işlemini CPU yoğun olduğu için ayrı bir task'te yap
        let resampled_samples_16khz = spawn_blocking(move || -> Result<Vec<i16>> {
            let pcm_f32: Vec<f32> = samples_from_file_8khz.iter().map(|&s| s as f32 / 32768.0).collect();
            let params = SincInterpolationParameters {
                sinc_len: 256, f_cutoff: 0.95, interpolation: SincInterpolationType::Linear,
                oversampling_factor: 256, window: WindowFunction::BlackmanHarris2,
            };
            let mut resampler = SincFixedIn::<f32>::new(16000.0 / 8000.0, 2.0, params, pcm_f32.len(), 1)?;
            let mut resampled_channels = resampler.process(&[pcm_f32], None)?;
            if resampled_channels.is_empty() { return Err(anyhow!("Resampler ses kanalı döndürmedi.")); }
            let resampled_f32 = resampled_channels.remove(0);
            Ok(resampled_f32.into_iter().map(|s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16).collect())
        }).await.context("Anonsu 16kHz'e yükseltme task'i başarısız oldu")??;
        
        info!(samples_count = resampled_samples_16khz.len(), "Anons 16kHz'e başarıyla yükseltildi.");
        return Ok(Arc::new(resampled_samples_16khz));
    }

    Err(anyhow!("Desteklenmeyen URI şeması: {}", uri))
}

// ======================== DÜZELTME SONU ========================


fn process_packet_task(packet_data: Vec<u8>) -> JoinHandle<Option<ProcessedAudio>> {
    spawn_blocking(move || {
        let mut packet_buf = &packet_data[..];
        if let Ok(packet) = Packet::unmarshal(&mut packet_buf) {
            if let Ok(incoming_codec) = AudioCodec::from_rtp_payload_type(packet.header.payload_type) {
                match codecs::decode_g711_to_lpcm16(&packet.payload, incoming_codec) {
                    Ok(samples_16khz) => Some(ProcessedAudio { samples_16khz, source_codec: incoming_codec }),
                    Err(e) => { error!(error = %e, "Ses verisi standart formata dönüştürülemedi."); None }
                }
            } else { None }
        } else { None }
    })
}


#[instrument(skip_all, fields(rtp_port = config.port))]
pub async fn rtp_session_handler(
    socket: Arc<UdpSocket>,
    mut command_rx: mpsc::Receiver<RtpCommand>,
    config: RtpSessionConfig,
) {
    info!("Yeni RTP oturumu dinleyicisi başlatıldı.");
    let mut actual_remote_addr: Option<SocketAddr> = None;
    let mut buf = [0u8; 2048];
    let mut current_playback_token: Option<CancellationToken> = None;
    let mut live_stream_sender: Option<mpsc::Sender<Result<AudioFrame, Status>>> = None;
    let mut permanent_recording_session: Option<RecordingSession> = None;
    let mut outbound_codec: Option<AudioCodec> = None;
    let inactivity_timeout = config.app_config.rtp_session_inactivity_timeout;
    let mut last_activity = Instant::now();
    let mut processing_task: Option<JoinHandle<Option<ProcessedAudio>>> = None;
    
    loop {
        let timeout_check = sleep(inactivity_timeout);
        tokio::pin!(timeout_check);

        tokio::select! {
            biased;
            Some(command) = command_rx.recv() => {
                last_activity = Instant::now();
                match command {
                    RtpCommand::PlayAudioUri { audio_uri, candidate_target_addr, cancellation_token } => {
                        if let Some(token) = current_playback_token.take() { token.cancel(); }
                        current_playback_token = Some(cancellation_token.clone());
                        if actual_remote_addr.is_none() { actual_remote_addr = Some(candidate_target_addr); }
                        let target = actual_remote_addr.unwrap_or(candidate_target_addr);
                        let codec_to_use = outbound_codec.unwrap_or(AudioCodec::Pcmu);
                        
                        // DEĞİŞİKLİK: Yeni, akıllı yükleyici fonksiyonunu kullanıyoruz.
                        let samples_to_play_and_record = match load_and_resample_samples_from_uri(&audio_uri, &config.app_state, &config.app_config).await {
                            Ok(s) => Some(s),
                            Err(e) => { error!(error = ?e, "Çalınacak ses yüklenemedi."); None }
                        };
                        
                        if let Some(samples_16khz) = samples_to_play_and_record {
                            if let Some(session) = &mut permanent_recording_session {
                                info!(samples_count = samples_16khz.len(), "Giden ses (16kHz) kalıcı kayda ekleniyor.");
                                session.samples.extend_from_slice(&samples_16khz);
                            }
                            let socket_clone = socket.clone();
                            tokio::spawn(async move {
                                if let Err(e) = send_rtp_stream(&socket_clone, target, &samples_16khz, cancellation_token, codec_to_use).await {
                                    error!(error = ?e, "RTP stream gönderiminde hata oluştu.");
                                }
                            });
                        }
                    },
                    // ... (Diğer RtpCommand'ler aynı kalır) ...
                    RtpCommand::StartLiveAudioStream { stream_sender, target_sample_rate: _ } => {
                        live_stream_sender = Some(stream_sender);
                    },
                    RtpCommand::StartPermanentRecording(session) => {
                        permanent_recording_session = Some(session);
                    },
                    RtpCommand::StopLiveAudioStream => { live_stream_sender = None; },
                    RtpCommand::StopPermanentRecording { responder } => {
                        if let Some(session) = permanent_recording_session.take() {
                            let recording_uri = session.output_uri.clone();
                            let result = finalize_and_save_recording(session, config.app_state.clone()).await;
                            let _ = responder.send(result.map(|_| recording_uri).map_err(|e| e.to_string()));
                        } else {
                            let _ = responder.send(Err("Durdurulacak kayıt bulunamadı".to_string()));
                        }
                    },
                    RtpCommand::StopAudio => if let Some(token) = current_playback_token.take() { token.cancel(); },
                    RtpCommand::Shutdown => {
                        if let Some(token) = current_playback_token.take() { token.cancel(); }
                        if let Some(session) = permanent_recording_session.take() {
                            let app_state_clone = config.app_state.clone();
                            tokio::spawn(finalize_and_save_recording(session, app_state_clone));
                        }
                        if let Some(sender) = live_stream_sender.take() { drop(sender); }
                        break;
                    }
                }
            },
            result = async { processing_task.as_mut().unwrap().await }, if processing_task.is_some() => {
                processing_task = None;
                if let Ok(Some(processed_audio)) = result {
                    if outbound_codec.is_none() {
                        info!(codec = ?processed_audio.source_codec, "Gelen ilk pakete göre giden kodek ayarlandı.");
                        outbound_codec = Some(processed_audio.source_codec);
                    }
                    if let Some(sender) = &live_stream_sender {
                        let media_type = "audio/L16;rate=16000".to_string();
                        let mut bytes = Vec::with_capacity(processed_audio.samples_16khz.len() * 2);
                        for &sample in &processed_audio.samples_16khz { bytes.extend_from_slice(&sample.to_le_bytes()); }
                        let frame = AudioFrame { data: bytes.into(), media_type };
                        if sender.send(Ok(frame)).await.is_err() {
                            debug!("Canlı ses akışı alıcısı kapatılmış, stream durduruluyor.");
                            live_stream_sender = None;
                        }
                    }
                    if let Some(session) = &mut permanent_recording_session {
                        session.samples.extend_from_slice(&processed_audio.samples_16khz);
                    }
                }
            },
            result = socket.recv_from(&mut buf), if processing_task.is_none() => {
                last_activity = Instant::now();
                if let Ok((len, addr)) = result {
                    if actual_remote_addr.is_none() { info!(remote = %addr, "İlk RTP paketi alındı, hedef adres doğrulandı."); }
                    actual_remote_addr = Some(addr);
                    let packet_data = buf[..len].to_vec();
                    processing_task = Some(process_packet_task(packet_data));
                }
            },
            _ = &mut timeout_check => {
                if last_activity.elapsed() >= inactivity_timeout {
                    if let Some(sender) = live_stream_sender.take() {
                        info!("RTP akışında aktivite yok, canlı stream sonlandırılıyor.");
                        drop(sender);
                    }
                }
            }
        }
    }
    info!("RTP oturumu temizleniyor...");
    config.app_state.port_manager.remove_session(config.port).await;
    config.app_state.port_manager.quarantine_port(config.port).await;
    gauge!(ACTIVE_SESSIONS).decrement(1.0);
}


#[instrument(skip_all, fields(uri = %session.output_uri, samples = session.samples.len(), call_id = %session.call_id, trace_id = %session.trace_id))]
async fn finalize_and_save_recording(session: RecordingSession, app_state: AppState) -> Result<()> {
    // Bu fonksiyonun geri kalanı doğru çalışıyor ve değiştirilmesine gerek yok.
    info!("Kayıt sonlandırma ve kaydetme süreci başlatıldı.");
    if session.samples.is_empty() {
        warn!("Kaydedilecek ses verisi yok, boş dosya oluşturulmayacak.");
        return Ok(());
    }
    
    let call_id_for_event = session.call_id.clone();
    let trace_id_for_event = session.trace_id.clone();
    let output_uri_for_event = session.output_uri.clone();
    
    let result: Result<()> = async {
        let samples_16k = session.samples;
        let original_len = samples_16k.len();

        let samples_8k = spawn_blocking(move || -> Result<Vec<i16>> {
            let pcm_f32: Vec<f32> = samples_16k.iter().map(|&s| s as f32 / 32768.0).collect();
            let params = SincInterpolationParameters {
                sinc_len: 256, f_cutoff: 0.95, interpolation: SincInterpolationType::Linear,
                oversampling_factor: 256, window: WindowFunction::BlackmanHarris2,
            };
            let mut resampler = SincFixedIn::<f32>::new(
                8000.0 / 16000.0, 2.0, params, pcm_f32.len(), 1,
            )?;
            let mut resampled_channels = resampler.process(&[pcm_f32], None)?;
            if resampled_channels.is_empty() { return Err(anyhow!("Resampler ses kanalı döndürmedi.")); }
            let resampled_f32 = resampled_channels.remove(0);
            Ok(resampled_f32.into_iter().map(|s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16).collect())
        }).await.context("Downsampling task'i başarısız oldu")??;
        
        info!("Kayıt 16kHz'den 8kHz'e düşürüldü. Orjinal örnek: {}, Yeni örnek: {}", original_len, samples_8k.len());

        let mut spec_8k = session.spec;
        spec_8k.sample_rate = 8000;
        let wav_data = spawn_blocking(move || -> Result<Vec<u8>, hound::Error> {
            let mut buffer = Cursor::new(Vec::new());
            let mut writer = WavWriter::new(&mut buffer, spec_8k)?;
            for sample in samples_8k { writer.write_sample(sample)?; }
            writer.finalize()?;
            Ok(buffer.into_inner())
        }).await.context("WAV dosyası oluşturma task'i başarısız oldu")??;

        info!(wav_size_bytes = wav_data.len(), "WAV verisi başarıyla oluşturuldu.");
        
        let writer = writers::from_uri(&session.output_uri, &app_state, &app_state.port_manager.config.clone())
            .await.context("Kayıt yazıcısı (writer) oluşturulamadı")?;
        writer.write(wav_data).await.context("Veri S3'e veya dosyaya yazılamadı")?;
        Ok(())
    }.await;

    if result.is_ok() {
        info!("Çağrı kaydı başarıyla tamamlandı.");
        metrics::counter!("sentiric_media_recording_saved_total", "storage_type" => "s3").increment(1);
        if let Some(publisher) = &app_state.rabbitmq_publisher {
            let event_payload = serde_json::json!({
                "eventType": "call.recording.available", "traceId": trace_id_for_event, "callId": call_id_for_event,
                "recordingUri": output_uri_for_event, "timestamp": chrono::Utc::now().to_rfc3339()
            });
            if let Err(e) = publisher.basic_publish(
                rabbitmq::EXCHANGE_NAME, "call.recording.available", BasicPublishOptions::default(),
                event_payload.to_string().as_bytes(), BasicProperties::default().with_delivery_mode(2)
            ).await {
                error!(error = ?e, "call.recording.available olayı yayınlanamadı (publish hatası).");
            } else {
                info!("'call.recording.available' olayı başarıyla yayınlandı.");
            }
        } else {
            warn!("RabbitMQ publisher bulunamadığı için kayıt olayı yayınlanamadı.");
        }
    } else {
        error!(error = ?result.as_ref().err(), "Kayıt kaydetme görevi başarısız oldu.");
        metrics::counter!("sentiric_media_recording_failed_total", "storage_type" => "s3").increment(1);
    }
    result
}