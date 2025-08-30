// --- File: sentiric-media-service/src/rtp/session.rs ---
use crate::audio::AudioCache;
use crate::config::AppConfig;
use crate::metrics::ACTIVE_SESSIONS;
use crate::rtp::codecs::ULAW_TO_PCM;
use crate::rtp::command::{AudioFrame, RecordingSession, RtpCommand};
use crate::rtp::stream::{decode_audio_with_symphonia, send_announcement_from_uri};
use crate::rtp::writers;
use crate::state::PortManager;
use anyhow::{anyhow, Context, Result}; // anyhow::Context trait'ini doğru şekilde import ediyoruz
use base64::{engine::general_purpose, Engine}; // Sadece Engine trait'ini import ediyoruz
use bytes::Bytes;
use hound::WavWriter;
use metrics::gauge;
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use std::io::Cursor;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tonic::Status;
use tracing::{error, info, instrument, warn};

pub struct RtpSessionConfig {
    pub port_manager: PortManager,
    pub audio_cache: AudioCache,
    pub app_config: Arc<AppConfig>,
    pub port: u16,
}

// DÖNÜŞ TİPİ DEĞİŞTİ: Arc<Vec<i16>>
async fn load_samples_from_uri(
    uri: &str,
    cache: &AudioCache,
    config: &Arc<AppConfig>,
) -> Result<Arc<Vec<i16>>> {
    if let Some(path_part) = uri.strip_prefix("file://") {
        let mut final_path = std::path::PathBuf::from(&config.assets_base_path);
        final_path.push(path_part.trim_start_matches('/'));
        crate::audio::load_or_get_from_cache(cache, &final_path).await
    } else if uri.starts_with("data:") {
        let (_media_type, base64_data) = uri
            .strip_prefix("data:")
            .and_then(|s| s.split_once(";base64,"))
            .context("Geçersiz data URI formatı")?;
        let audio_bytes = general_purpose::STANDARD
            .decode(base64_data)
            .context("Base64 verisi çözümlenemedi")?;
        // DÖNÜŞ TİPİ DEĞİŞTİ: Arc::new() ile sarmalıyoruz
        Ok(Arc::new(decode_audio_with_symphonia(audio_bytes)?))
    } else {
        Err(anyhow!("Desteklenmeyen URI şeması: {}", uri))
    }
}


#[instrument(skip_all, fields(rtp_port = config.port))]
pub async fn rtp_session_handler(
    socket: Arc<UdpSocket>,
    mut rx: mpsc::Receiver<RtpCommand>,
    config: RtpSessionConfig,
) {
    info!("Yeni RTP oturumu dinleyicisi başlatıldı.");
    
    let mut actual_remote_addr: Option<SocketAddr> = None;
    let mut buf = [0u8; 2048];
    let mut current_playback_token: Option<CancellationToken> = None;
    let mut live_stream_sender: Option<(mpsc::Sender<Result<AudioFrame, Status>>, Option<u32>)> = None;
    let mut permanent_recording_session: Option<RecordingSession> = None;

    loop {
        tokio::select! {
            biased;
            Some(command) = rx.recv() => {
                match command {
                    RtpCommand::PlayAudioUri { audio_uri, candidate_target_addr, cancellation_token } => {
                        if let Some(token) = current_playback_token.take() {
                            info!("Devam eden bir anons var, iptal ediliyor.");
                            token.cancel();
                        }
                        current_playback_token = Some(cancellation_token.clone());
                        let target = actual_remote_addr.unwrap_or(candidate_target_addr);

                        if let Some(rec_session) = &mut permanent_recording_session {
                            info!("Giden ses (outbound audio) kalıcı kayda ekleniyor.");
                            match load_samples_from_uri(&audio_uri, &config.audio_cache, &config.app_config).await {
                                Ok(samples) => {
                                    rec_session.samples.extend_from_slice(&samples);
                                    info!(samples_added = samples.len(), "Giden ses örnekleri kayda eklendi.");
                                },
                                Err(e) => {
                                    error!(error = ?e, uri = %audio_uri, "Kayıt için giden ses yüklenemedi.");
                                }
                            }
                        }
                        
                        tokio::spawn(send_announcement_from_uri(
                            socket.clone(),
                            target,
                            audio_uri,
                            config.audio_cache.clone(),
                            config.app_config.clone(),
                            cancellation_token,
                        ));
                    },
                    RtpCommand::StartLiveAudioStream { stream_sender, target_sample_rate } => {
                        info!(target_rate = ?target_sample_rate, "Gerçek zamanlı ses akışı komutu alındı.");
                        live_stream_sender = Some((stream_sender, target_sample_rate));
                    },
                    RtpCommand::StopLiveAudioStream => {
                        if live_stream_sender.is_some() {
                            info!("Gerçek zamanlı ses akışı komutla durduruldu.");
                            live_stream_sender = None;
                        }
                    },
                    RtpCommand::StartPermanentRecording(session) => {
                        info!(uri = %session.output_uri, "Kalıcı kayıt başlatılıyor...");
                        permanent_recording_session = Some(session);
                    },
                    RtpCommand::StopPermanentRecording => {
                        info!("Kalıcı kayıt durduruluyor...");
                        if let Some(session) = permanent_recording_session.take() {
                            tokio::spawn(finalize_and_save_recording(session, config.app_config.clone()));
                        } else {
                            warn!("Durdurulacak aktif bir kalıcı kayıt bulunamadı.");
                        }
                    },
                    RtpCommand::StopAudio => {
                        if let Some(token) = current_playback_token.take() {
                            info!("Anons çalma komutu dışarıdan durduruldu.");
                            token.cancel();
                        }
                    },
                    RtpCommand::Shutdown => {
                        info!("Shutdown komutu alındı, oturum sonlandırılıyor.");
                        if let Some(token) = current_playback_token.take() { token.cancel(); }
                        if let Some(session) = permanent_recording_session.take() {
                            tokio::spawn(finalize_and_save_recording(session, config.app_config.clone()));
                        }
                        if let Some((sender, _)) = live_stream_sender.take() { drop(sender); }
                        break;
                    }
                }
            },
            result = socket.recv_from(&mut buf) => {
                match result {
                    Ok((len, addr)) => {
                        if len <= 12 { continue; }

                        if actual_remote_addr.is_none() {
                            info!(remote = %addr, "İlk RTP paketi alındı, hedef adres doğrulandı.");
                            actual_remote_addr = Some(addr);
                        }
                        
                        let pcmu_payload = &buf[12..len];
                        
                        if let Some((sender, target_rate_opt)) = &live_stream_sender {
                            match process_audio_chunk(pcmu_payload, *target_rate_opt) {
                                Ok((pcm_bytes, _)) => {
                                    let media_type = format!("audio/L16;rate={}", target_rate_opt.unwrap_or(8000));
                                    let audio_frame = AudioFrame { data: pcm_bytes, media_type };
                                    
                                    if sender.send(Ok(audio_frame)).await.is_err() {
                                        info!("gRPC stream alıcısı kapandı, canlı ses akışı durduruldu.");
                                        live_stream_sender = None;
                                    }
                                },
                                Err(e) => error!(error = %e, "Ses verisi işlenemedi."),
                            }
                        }

                        if let Some(session) = &mut permanent_recording_session {
                             match process_audio_chunk(pcmu_payload, Some(session.spec.sample_rate)) {
                                Ok((_, pcm_samples_i16)) => {
                                    session.samples.extend_from_slice(&pcm_samples_i16);
                                }
                                Err(e) => error!(error = %e, "Kalıcı kayıt için ses verisi işlenemedi."),
                            }
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "Socket recv_from hatası");
                    }
                }
            },
        }
    }
    
    info!("RTP oturumu temizleniyor...");
    config.port_manager.remove_session(config.port).await;
    config.port_manager.quarantine_port(config.port).await;
    gauge!(ACTIVE_SESSIONS).decrement(1.0);
}

#[instrument(skip_all, fields(uri = %session.output_uri, samples = session.samples.len()))]
async fn finalize_and_save_recording(session: RecordingSession, config: Arc<AppConfig>) {
    info!("Kayıt sonlandırma ve kaydetme süreci başlatıldı.");

    if session.samples.is_empty() {
        warn!("Kaydedilecek ses verisi yok, boş dosya oluşturulmayacak.");
        return;
    }

    let result: Result<(), anyhow::Error> = async {
        let wav_data = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, hound::Error> {
            let mut buffer = Cursor::new(Vec::new());
            let mut writer = WavWriter::new(&mut buffer, session.spec)?;
            for sample in session.samples {
                writer.write_sample(sample)?;
            }
            writer.finalize()?;
            Ok(buffer.into_inner())
        }).await.context("WAV dosyası oluşturma task'i başarısız oldu")??;

        info!(wav_size_bytes = wav_data.len(), "WAV verisi başarıyla oluşturuldu.");

        let writer = writers::from_uri(&session.output_uri, &config)
            .await
            .context("Kayıt yazıcısı (writer) oluşturulamadı")?;
        
        info!("Kayıt yazıcısı oluşturuldu, veriler yazılıyor...");

        writer.write(wav_data)
            .await
            .context("Veri S3'e veya dosyaya yazılamadı")?;
        
        Ok(())
    }.await;

    match result {
        Ok(_) => {
            info!("Çağrı kaydı başarıyla tamamlandı ve kaydedildi.");
            metrics::counter!("sentiric_media_recording_saved_total", "storage_type" => "s3").increment(1);
        }
        Err(e) => {
            error!(error = ?e, "Kayıt kaydetme görevi başarısız oldu.");
            metrics::counter!("sentiric_media_recording_failed_total", "storage_type" => "s3").increment(1);
        }
    }
}

fn process_audio_chunk(pcmu_payload: &[u8], target_sample_rate: Option<u32>) -> Result<(Bytes, Vec<i16>), anyhow::Error> {
    const SOURCE_SAMPLE_RATE: u32 = 8000;
    
    let pcm_samples_i16: Vec<i16> = pcmu_payload.iter()
        .map(|&byte| ULAW_TO_PCM[byte as usize])
        .collect();
    
    let target_rate = target_sample_rate.unwrap_or(SOURCE_SAMPLE_RATE);

    if target_rate == SOURCE_SAMPLE_RATE {
        let mut bytes = Vec::with_capacity(pcm_samples_i16.len() * 2);
        for &sample in &pcm_samples_i16 {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        return Ok((Bytes::from(bytes), pcm_samples_i16));
    }

    let pcm_f32: Vec<f32> = pcm_samples_i16.iter()
        .map(|&sample| sample as f32 / 32768.0)
        .collect();

    let params = SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 256,
        window: WindowFunction::BlackmanHarris2,
    };
    
    let mut resampler = SincFixedIn::<f32>::new(
        target_rate as f64 / SOURCE_SAMPLE_RATE as f64,
        2.0,
        params,
        pcm_f32.len(),
        1,
    )?;
    
    let resampled_f32 = resampler.process(&[pcm_f32], None)?.remove(0);
    
    let resampled_i16: Vec<i16> = resampled_f32.into_iter()
        .map(|s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16)
        .collect();

    let mut bytes = Vec::with_capacity(resampled_i16.len() * 2);
    for &sample in &resampled_i16 {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    
    Ok((Bytes::from(bytes), resampled_i16))
}