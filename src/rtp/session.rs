// File: src/rtp/session.rs (TAM VE EKSİKSİZ NİHAİ HALİ)
use crate::config::AppConfig;
use crate::metrics::ACTIVE_SESSIONS;
use crate::rtp::command::{AudioFrame, RecordingSession, RtpCommand};
use crate::rtp::stream::{decode_audio_with_symphonia, send_rtp_stream};
use crate::rtp::writers;
use crate::state::AppState;
use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose, Engine};
use hound::{WavWriter, WavSpec};
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
use crate::rtp::codecs::{self, AudioCodec};
use crate::audio::AudioCache;
use rubato::Resampler;

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

async fn load_samples_from_uri(
    uri: &str,
    app_state: &AppState,
    config: &Arc<AppConfig>,
) -> Result<Arc<Vec<i16>>> {
    if let Some(path_part) = uri.strip_prefix("file://") {
        let mut final_path = PathBuf::from(&config.assets_base_path);
        final_path.push(path_part.trim_start_matches('/'));
        load_wav_and_resample(&app_state.audio_cache, &final_path).await
    } else if uri.starts_with("data:") {
        info!("Data URI'sinden ses yükleniyor...");
        let (_media_type, base64_data) = uri
            .strip_prefix("data:")
            .and_then(|s| s.split_once(";base64,"))
            .context("Geçersiz data URI formatı")?;
        let audio_bytes = general_purpose::STANDARD
            .decode(base64_data)
            .context("Base64 verisi çözümlenemedi")?;
        Ok(Arc::new(decode_audio_with_symphonia(audio_bytes)?))
    } else {
        Err(anyhow!("Desteklenmeyen URI şeması: {}", uri))
    }
}

async fn load_wav_and_resample(cache: &AudioCache, path: &PathBuf) -> Result<Arc<Vec<i16>>> {
    let path_key = path.to_string_lossy().to_string();
    let mut cache_guard = cache.lock().await;

    if let Some(cached) = cache_guard.get(&path_key) {
        debug!(path = %path.display(), "Ses önbellekten okundu.");
        return Ok(cached.clone());
    }
    
    info!(path = %path.display(), "Ses diskten okunuyor ve işleniyor...");
    let path_owned = path.clone();
    let (spec, samples_i16) = spawn_blocking(move || -> Result<(WavSpec, Vec<i16>)> {
        let mut reader = hound::WavReader::open(path_owned)?;
        let spec = reader.spec();
        let samples = reader.samples::<i16>().collect::<Result<Vec<_>, _>>()?;
        Ok((spec, samples))
    }).await??;

    if spec.sample_rate == crate::rtp::stream::INTERNAL_SAMPLE_RATE {
        let samples_arc = Arc::new(samples_i16);
        cache_guard.insert(path_key, samples_arc.clone());
        return Ok(samples_arc);
    }

    info!(from = spec.sample_rate, to = crate::rtp::stream::INTERNAL_SAMPLE_RATE, "Anons dosyası yeniden örnekleniyor...");
    let samples_f32: Vec<f32> = samples_i16.iter().map(|&s| s as f32 / 32768.0).collect();
    let params = rubato::SincInterpolationParameters {
        sinc_len: 256, f_cutoff: 0.95, interpolation: rubato::SincInterpolationType::Linear,
        oversampling_factor: 256, window: rubato::WindowFunction::BlackmanHarris2,
    };
    let mut resampler = rubato::SincFixedIn::<f32>::new(
        crate::rtp::stream::INTERNAL_SAMPLE_RATE as f64 / spec.sample_rate as f64,
        2.0, params, samples_f32.len(), 1,
    )?;
    let resampled = resampler.process(&[samples_f32], None)?.remove(0);
    let final_samples_i16: Vec<i16> = resampled.into_iter().map(|s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16).collect();

    let samples_arc = Arc::new(final_samples_i16);
    cache_guard.insert(path_key, samples_arc.clone());
    info!(path = %path.display(), "Ses önbelleğe alındı (yeniden örneklendi).");
    Ok(samples_arc)
}

fn process_packet_task(packet_data: Vec<u8>) -> JoinHandle<Option<ProcessedAudio>> {
    spawn_blocking(move || {
        let mut packet_buf = &packet_data[..];
        if let Ok(packet) = Packet::unmarshal(&mut packet_buf) {
            if let Ok(incoming_codec) = AudioCodec::from_rtp_payload_type(packet.header.payload_type) {
                match codecs::decode_g711_to_lpcm16(&packet.payload, incoming_codec) {
                    Ok(samples_16khz) => Some(ProcessedAudio { samples_16khz, source_codec: incoming_codec }),
                    Err(e) => {
                        error!(error = %e, "Ses verisi standart formata dönüştürülemedi.");
                        None
                    }
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
                        let app_state_clone = config.app_state.clone();
                        let config_clone = config.app_config.clone();
                        let socket_clone = socket.clone();
                        
                        let samples_to_play_and_record = match load_samples_from_uri(&audio_uri, &app_state_clone, &config_clone).await {
                            Ok(s) => Some(s),
                            Err(e) => {
                                error!(error = ?e, "Çalınacak ses yüklenemedi.");
                                None
                            }
                        };
                        
                        if let Some(samples) = samples_to_play_and_record {
                            // === DEĞİŞİKLİK BURADA: Klonla ve `move` et ===
                            let samples_clone_for_recording = samples.clone();
                            if let Some(session) = &mut permanent_recording_session {
                                info!(samples_count = samples_clone_for_recording.len(), "Giden anons kalıcı kayda ekleniyor.");
                                session.samples.extend_from_slice(&samples_clone_for_recording);
                            }

                            tokio::spawn(async move {
                                if let Err(e) = send_rtp_stream(&socket_clone, target, &samples, cancellation_token, codec_to_use).await {
                                    error!(error = ?e, "RTP stream gönderiminde hata oluştu.");
                                }
                            });
                        }
                    },
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
                            let result = finalize_and_save_recording(session, config.app_state.clone(), config.app_config.clone()).await;
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
                            let app_config_clone = config.app_config.clone();
                            tokio::spawn(finalize_and_save_recording(session, app_state_clone, app_config_clone));
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
                        for &sample in &processed_audio.samples_16khz {
                            bytes.extend_from_slice(&sample.to_le_bytes());
                        }
                        let frame = AudioFrame { data: bytes.into(), media_type };
                        if sender.send(Ok(frame)).await.is_err() {
                            debug!("Canlı ses akışı alıcısı kapatılmış, stream durduruluyor.");
                            live_stream_sender = None;
                        }
                    }
                    // === BU SATIR KRİTİK: GELEN SESİ KAYDA EKLE ===
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

#[instrument(skip_all, fields(uri = %session.output_uri, samples = session.samples.len()))]
async fn finalize_and_save_recording(
    session: RecordingSession,
    app_state: AppState,
    config: Arc<AppConfig>,
) -> Result<()> {
    info!("Kayıt sonlandırma ve kaydetme süreci başlatıldı.");
    if session.samples.is_empty() {
        warn!("Kaydedilecek ses verisi yok, boş dosya oluşturulmayacak.");
        return Ok(());
    }
    let result: Result<()> = async {
        let wav_data = spawn_blocking(move || -> Result<Vec<u8>, hound::Error> {
            let mut buffer = Cursor::new(Vec::new());
            let spec_16khz = hound::WavSpec {
                channels: 1, sample_rate: 16000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int,
            };
            let mut writer = WavWriter::new(&mut buffer, spec_16khz)?;
            for sample in session.samples { writer.write_sample(sample)?; }
            writer.finalize()?;
            Ok(buffer.into_inner())
        }).await.context("WAV dosyası oluşturma task'i başarısız oldu")??;
        
        info!(wav_size_bytes = wav_data.len(), "WAV verisi başarıyla oluşturuldu.");
        let writer = writers::from_uri(&session.output_uri, &app_state, &config).await
            .context("Kayıt yazıcısı (writer) oluşturulamadı")?;
        writer.write(wav_data).await.context("Veri S3'e veya dosyaya yazılamadı")?;
        Ok(())
    }.await;
    if result.is_ok() {
        info!("Çağrı kaydı başarıyla tamamlandı.");
        metrics::counter!("sentiric_media_recording_saved_total", "storage_type" => "s3").increment(1);

        // === YENİ GÖREV (MEDIA-004): KAYIT TAMAMLANDI OLAYINI YAYINLA ===
        // Bu kod bloğunu ekleyeceğiz, ancak bunun çalışması için `AppState`'e
        // RabbitMQ channel'ını da eklememiz gerekecek. Şimdilik konsept olarak ekliyorum.
        // TODO: AppState'e RabbitMQ channel ekle ve bu olayı yayınla.
        /*
        if let Some(publisher) = &app_state.rabbitmq_publisher {
            let event = CallRecordingAvailableEvent { ... };
            if let Err(e) = publisher.publish(event).await {
                error!(error = %e, "call.recording.available olayı yayınlanamadı.");
            }
        }
        */

    }  else {
        error!(error = ?result.as_ref().err(), "Kayıt kaydetme görevi başarısız oldu.");
        metrics::counter!("sentiric_media_recording_failed_total", "storage_type" => "s3").increment(1);
    }
    result
}