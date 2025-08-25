use std::net::SocketAddr;
use std::sync::Arc;
use std::env;
use std::path::Path;
use std::io::Cursor;


use tokio::net::UdpSocket;
use tokio::sync::{mpsc, Mutex};
use tokio_util::sync::CancellationToken;
use tonic::Status;
use tracing::{error, info, instrument, warn};
use bytes::{Bytes, BytesMut};
use hound::{WavWriter, WavSpec};
use metrics::gauge;

use crate::audio::AudioCache;
use crate::config::AppConfig;
use crate::rtp::command::{RtpCommand, AudioFrame, RecordingSession};
use crate::rtp::stream::send_announcement_from_uri;
use crate::state::PortManager;
use rubato::{Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};

use crate::rtp::codecs::ULAW_TO_PCM;
use crate::metrics::ACTIVE_SESSIONS;

pub struct RtpSessionConfig {
    pub port_manager: PortManager,
    pub audio_cache: AudioCache,
    pub app_config: Arc<AppConfig>,
    pub port: u16,
}

#[instrument(skip_all, fields(rtp_port = config.port))]
pub async fn rtp_session_handler(
    socket: Arc<UdpSocket>,
    mut rx: mpsc::Receiver<RtpCommand>,
    config: RtpSessionConfig,
) {
    info!("Yeni RTP oturumu dinleyicisi başlatıldı.");
    
    let mut debug_wav_writer = None;
    let mut debug_wav_filename = String::new();

    if !config.app_config.debug_wav_path_template.is_empty() {
        let temp_dir_str = env::temp_dir().to_string_lossy().to_string();
        debug_wav_filename = config.app_config.debug_wav_path_template
            .replace("{temp}", &temp_dir_str)
            .replace("{port}", &config.port.to_string());
        
        if let Some(parent_dir) = Path::new(&debug_wav_filename).parent() {
            if !parent_dir.exists() {
                if let Err(e) = std::fs::create_dir_all(parent_dir) {
                    error!(error = %e, path = ?parent_dir, "Debug WAV dizini oluşturulamadı.");
                }
            }
        }
        
        if Path::new(&debug_wav_filename).parent().map_or(false, |p| p.exists()) {
            let spec = WavSpec {
                channels: 1,
                sample_rate: config.app_config.debug_wav_sample_rate,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            match WavWriter::create(&debug_wav_filename, spec) {
                Ok(writer) => {
                    info!(filename = %debug_wav_filename, "Debug: Gelen ve işlenen ses bu dosyaya kaydedilecek.");
                    debug_wav_writer = Some(Arc::new(Mutex::new(Some(writer))));
                }
                Err(e) => {
                    error!(error = %e, filename = %debug_wav_filename, "Debug WAV dosyası oluşturulamadı");
                }
            };
        }
    }

    let mut actual_remote_addr: Option<SocketAddr> = None;
    let mut buf = [0u8; 2048];
    let mut current_playback_token: Option<CancellationToken> = None;
    let mut streaming_sender: Option<(mpsc::Sender<Result<AudioFrame, Status>>, Option<u32>)> = None;
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
                        tokio::spawn(send_announcement_from_uri(
                            socket.clone(),
                            target,
                            audio_uri,
                            config.audio_cache.clone(),
                            config.app_config.clone(),
                            cancellation_token,
                        ));
                    },
                    RtpCommand::StartRecording { stream_sender, target_sample_rate } => {
                        info!(target_rate = ?target_sample_rate, "Gerçek zamanlı ses akışı komutu alındı.");
                        streaming_sender = Some((stream_sender, target_sample_rate));
                    },
                    RtpCommand::StartPermanentRecording(session) => {
                        info!(uri = %session.output_uri, "Kalıcı kayıt başlatılıyor...");
                        permanent_recording_session = Some(session);
                    },
                    RtpCommand::StopPermanentRecording => {
                        info!("Kalıcı kayıt durduruluyor...");
                        if let Some(session) = permanent_recording_session.take() {
                            tokio::spawn(finalize_and_save_recording(session));
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
                        if let Some(token) = current_playback_token.take() {
                            token.cancel();
                        }
                        if let Some(session) = permanent_recording_session.take() {
                            tokio::spawn(finalize_and_save_recording(session));
                        }
                        if let Some((sender, _)) = streaming_sender.take() {
                            drop(sender);
                        }
                        break;
                    }
                }
            },
            result = socket.recv_from(&mut buf) => {
                match result {
                    Ok((len, addr)) => {
                        let read_buf = BytesMut::from(&buf[..len]);
                        let decrypted_len = read_buf.len();
                        if decrypted_len <= 12 { continue; }

                        if actual_remote_addr.is_none() {
                            info!(remote = %addr, "İlk RTP paketi alındı, hedef adres doğrulandı.");
                            actual_remote_addr = Some(addr);
                        }
                        
                        let pcmu_payload = &read_buf[12..decrypted_len];
                        
                        if let Some((sender, target_rate_opt)) = &streaming_sender {
                            match process_audio_chunk(pcmu_payload, *target_rate_opt) {
                                Ok((pcm_bytes, _)) => {
                                    let audio_frame = AudioFrame {
                                        data: pcm_bytes,
                                        media_type: if let Some(rate) = target_rate_opt {
                                            format!("audio/l16;rate={}", rate)
                                        } else {
                                            "audio/pcmu".to_string()
                                        },
                                    };
                                    if sender.send(Ok(audio_frame)).await.is_err() {
                                        warn!("Canlı akış istemcisi kapandı, akış durduruluyor.");
                                        streaming_sender = None;
                                    }
                                },
                                Err(e) => error!(error = %e, "Canlı akış için ses verisi işlenemedi."),
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

                        if let Some(writer_arc) = &debug_wav_writer {
                            match process_audio_chunk(pcmu_payload, Some(config.app_config.debug_wav_sample_rate)) {
                                Ok((_, pcm_samples_i16)) => {
                                    let wav_writer_clone = writer_arc.clone();
                                    tokio::spawn(async move {
                                        let mut writer_guard = wav_writer_clone.lock().await;
                                        if let Some(writer) = writer_guard.as_mut() {
                                            for sample in &pcm_samples_i16 {
                                                if let Err(e) = writer.write_sample(*sample) {
                                                    error!(error = %e, "Debug WAV yazma hatası");
                                                    break;
                                                }
                                            }
                                        }
                                    });
                                },
                                Err(e) => error!(error = %e, "Debug kaydı için ses verisi işlenemedi."),
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
    
    if let Some(writer_arc) = debug_wav_writer {
        let mut writer_guard = writer_arc.lock().await;
        if let Some(writer) = writer_guard.take() {
            match writer.finalize() {
                Ok(_) => info!(filename = %debug_wav_filename, "Debug: Ses kaydı tamamlandı ve dosya kapatıldı."),
                Err(e) => error!(error = %e, "Debug WAV dosyası finalize edilemedi."),
            }
        }
    }
    
    info!("RTP oturumu temizleniyor...");
    config.port_manager.remove_session(config.port).await;
    config.port_manager.quarantine_port(config.port).await;
    gauge!(ACTIVE_SESSIONS).decrement(1.0);
}

fn save_recording(session: RecordingSession) -> Result<(), anyhow::Error> {
    info!(uri = %session.output_uri, samples_count = session.samples.len(), "Kayıt sonlandırılıyor ve kaydediliyor...");
    
    if session.samples.is_empty() {
        warn!("Kaydedilecek ses verisi yok, boş dosya oluşturulmayacak.");
        return Ok(());
    }

    let mut buffer = Cursor::new(Vec::new());
    let mut writer = WavWriter::new(&mut buffer, session.spec)?;
    for sample in session.samples {
        writer.write_sample(sample)?;
    }
    writer.finalize()?;
    
    if let Some(path_part) = session.output_uri.strip_prefix("file://") {
        let path = Path::new(path_part);
        if let Some(parent_dir) = path.parent() {
            std::fs::create_dir_all(parent_dir)?;
        }
        std::fs::write(path, buffer.into_inner())?;
        info!(path = %path.display(), "Kayıt dosyası başarıyla diske yazıldı.");

    } else if let Some(_) = session.output_uri.strip_prefix("s3://") {
         warn!("S3 yüklemesi henüz implemente edilmedi.");
    } else {
        error!(uri = %session.output_uri, "Desteklenmeyen kayıt URI şeması.");
    }

    Ok(())
}

async fn finalize_and_save_recording(session: RecordingSession) {
    if let Err(e) = tokio::task::spawn_blocking(move || save_recording(session)).await {
        error!(error = ?e, "Kayıt kaydetme görevi panic'ledi.");
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