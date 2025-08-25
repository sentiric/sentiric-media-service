// File: sentiric-media-service/src/rtp/session.rs
use std::net::SocketAddr;
use std::sync::Arc;
use std::env;
// YENİ: Dosya sistemi işlemleri için
use std::fs;
use std::path::Path;

use tokio::net::UdpSocket;
use tokio::sync::{mpsc, Mutex};
use tokio_util::sync::CancellationToken;
use tonic::Status;
use tracing::{error, info, instrument, warn};
use bytes::Bytes;
use hound::{WavWriter, WavSpec};

use crate::audio::AudioCache;
use crate::config::AppConfig;
use crate::rtp::command::{RtpCommand, AudioFrame};
use crate::rtp::stream::send_announcement_from_uri;
use crate::state::PortManager;
use rubato::{Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};

use crate::rtp::codecs::ULAW_TO_PCM;

#[instrument(skip_all, fields(rtp_port = port))]
pub async fn rtp_session_handler(
    socket: Arc<UdpSocket>,
    mut rx: mpsc::Receiver<RtpCommand>,
    port_manager: PortManager,
    audio_cache: AudioCache,
    config: Arc<AppConfig>,
    port: u16,
) {
    info!("Yeni RTP oturumu dinleyicisi başlatıldı.");
    
    let mut wav_writer = None;
    let mut wav_filename = String::new();

    // YENİ: Sadece path template'i boş DEĞİLSE kayıt yap.
    if !config.debug_wav_path_template.is_empty() {
        // {temp} ve {port} yer tutucularını değiştir
        let temp_dir_str = env::temp_dir().to_string_lossy().to_string();
        wav_filename = config.debug_wav_path_template
            .replace("{temp}", &temp_dir_str)
            .replace("{port}", &port.to_string());
        
        // Dosyanın yazılacağı dizinin var olduğundan emin ol
        if let Some(parent_dir) = Path::new(&wav_filename).parent() {
            if !parent_dir.exists() {
                if let Err(e) = fs::create_dir_all(parent_dir) {
                    error!(error = %e, path = ?parent_dir, "Debug WAV dizini oluşturulamadı.");
                    // Hata durumunda wav_writer None olarak kalacak ve program devam edecek.
                }
            }
        }
        
        // Sadece dizin oluşturma başarılıysa dosyayı oluşturmayı dene
        if Path::new(&wav_filename).parent().map_or(false, |p| p.exists()) {
            let spec = WavSpec {
                channels: 1,
                sample_rate: config.debug_wav_sample_rate,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            
            match WavWriter::create(&wav_filename, spec) {
                Ok(writer) => {
                    info!(filename = %wav_filename, "Debug: Gelen ve işlenen ses bu dosyaya kaydedilecek.");
                    wav_writer = Some(Arc::new(Mutex::new(Some(writer))));
                }
                Err(e) => {
                    error!(error = %e, filename = %wav_filename, "Debug WAV dosyası oluşturulamadı");
                }
            };
        }
    }
    
    let mut actual_remote_addr: Option<SocketAddr> = None;
    let mut buf = [0u8; 2048];
    let mut current_playback_token: Option<CancellationToken> = None;
    let mut recording_sender: Option<(mpsc::Sender<Result<AudioFrame, Status>>, Option<u32>)> = None;

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
                            audio_cache.clone(),
                            config.clone(),
                            cancellation_token
                        ));
                    },
                    RtpCommand::StartRecording { stream_sender, target_sample_rate } => {
                        info!(target_rate = ?target_sample_rate, "Ses kaydı komutu alındı.");
                        recording_sender = Some((stream_sender, target_sample_rate));
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
                        if let Some((sender, _)) = recording_sender.take() {
                            drop(sender);
                        }
                        break;
                    }
                }
            },
            result = socket.recv_from(&mut buf) => {
                match result {
                    Ok((len, addr)) => {
                        if len <= 12 {
                            continue;
                        }

                        if actual_remote_addr.is_none() {
                            info!(remote = %addr, "İlk RTP paketi alındı, hedef adres doğrulandı.");
                            actual_remote_addr = Some(addr);
                        }

                        if let Some((sender, target_rate_opt)) = &recording_sender {
                            let pcmu_payload = &buf[12..len];
                            
                            // YAZIM HATASI DÜZELTMESİ (ÖNCEKİ ADIMDAN)
                            let audio_frame_result = process_audio_chunk(pcmu_payload, *target_rate_opt);
                            
                            match audio_frame_result {
                                Ok((pcm_bytes, pcm_samples_i16)) => {
                                    // YENİ: Sadece wav_writer varsa yazma işlemini yap
                                    if let Some(writer_arc) = &wav_writer {
                                        let wav_writer_clone = writer_arc.clone();
                                        tokio::spawn(async move {
                                            let mut writer_guard = wav_writer_clone.lock().await;
                                            if let Some(writer) = writer_guard.as_mut() {
                                                for sample in &pcm_samples_i16 {
                                                    if let Err(e) = writer.write_sample(*sample) {
                                                        error!(error = %e, "WAV yazma hatası");
                                                        break;
                                                    }
                                                }
                                            }
                                        });
                                    }

                                    let audio_frame = AudioFrame {
                                        data: pcm_bytes,
                                        media_type: if let Some(rate) = target_rate_opt {
                                            format!("audio/l16;rate={}", rate)
                                        } else {
                                            "audio/pcmu".to_string()
                                        },
                                    };

                                    if sender.send(Ok(audio_frame)).await.is_err() {
                                        warn!("Kayıt stream'i istemci tarafından kapatıldı, kayıt durduruluyor.");
                                        recording_sender = None;
                                    }
                                },
                                Err(e) => {
                                    error!(error = %e, "Ses verisi işlenemedi.");
                                }
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
    
    // YENİ: Sadece wav_writer varsa finalize et
    if let Some(writer_arc) = wav_writer {
        let mut writer_guard = writer_arc.lock().await;
        if let Some(writer) = writer_guard.take() {
            match writer.finalize() {
                Ok(_) => info!(filename = %wav_filename, "Debug: Ses kaydı tamamlandı ve dosya kapatıldı."),
                Err(e) => error!(error = %e, "Debug WAV dosyası finalize edilemedi."),
            }
        }
    }
    
    info!("RTP oturumu temizleniyor...");
    port_manager.remove_session(port).await;
    port_manager.quarantine_port(port).await;
}

// YAZIM HATASI DÜZELTMESİ (ÖNCEKİ ADIMDAN)
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