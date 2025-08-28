// File: src/rtp/session.rs (GÜNCELLENMİŞ)

// ... (use ifadeleri ve diğer struct'lar aynı kalacak) ...
use std::net::SocketAddr;
use std::sync::Arc;
use std::io::Cursor;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tonic::Status;
use tracing::{error, info, instrument, warn};
use bytes::Bytes;
use hound::WavWriter;
use metrics::gauge;
use rubato::{Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};
use crate::audio::AudioCache;
use crate::config::AppConfig;
use crate::rtp::command::{RtpCommand, AudioFrame, RecordingSession};
use crate::rtp::stream::send_announcement_from_uri;
use crate::state::PortManager;
use crate::rtp::codecs::ULAW_TO_PCM;
use crate::metrics::ACTIVE_SESSIONS;
use crate::rtp::writers;

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
    
    let mut actual_remote_addr: Option<SocketAddr> = None;
    let mut buf = [0u8; 2048];
    let mut current_playback_token: Option<CancellationToken> = None;
    
    // YENİ: Canlı ses akışının gönderileceği kanalı ve ayarları tutacak değişken.
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
                        tokio::spawn(send_announcement_from_uri(
                            socket.clone(),
                            target,
                            audio_uri,
                            config.audio_cache.clone(),
                            config.app_config.clone(),
                            cancellation_token,
                        ));
                    },

                    // YENİ: Canlı akış başlatma komutunu işle.
                    RtpCommand::StartLiveAudioStream { stream_sender, target_sample_rate } => {
                        info!(target_rate = ?target_sample_rate, "Gerçek zamanlı ses akışı komutu alındı.");
                        // Varsa eski stream'i durdurup yenisiyle değiştiriyoruz.
                        live_stream_sender = Some((stream_sender, target_sample_rate));
                    },

                    // YENİ: Canlı akış durdurma komutunu işle.
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
                        // ÖNEMLİ: Kapatma sinyali geldiğinde stream sender'ı düşürerek
                        // gRPC stream'inin de sonlanmasını sağlıyoruz.
                        if let Some((sender, _)) = live_stream_sender.take() { drop(sender); }
                        break;
                    }
                }
            },
            result = socket.recv_from(&mut buf) => {
                match result {
                    Ok((len, addr)) => {
                        // RTP başlığı 12 byte, payload ondan sonra başlar.
                        if len <= 12 { continue; }

                        if actual_remote_addr.is_none() {
                            info!(remote = %addr, "İlk RTP paketi alındı, hedef adres doğrulandı.");
                            actual_remote_addr = Some(addr);
                        }
                        
                        let pcmu_payload = &buf[12..len];
                        
                        // YENİ: Eğer bir canlı stream dinleyicisi varsa, veriyi işle ve gönder.
                        if let Some((sender, target_rate_opt)) = &live_stream_sender {
                            match process_audio_chunk(pcmu_payload, *target_rate_opt) {
                                Ok((pcm_bytes, _)) => {
                                    // Örnek bir media_type, target_rate'e göre dinamik olabilir.
                                    let media_type = format!("audio/L16;rate={}", target_rate_opt.unwrap_or(8000));
                                    let audio_frame = AudioFrame { data: pcm_bytes, media_type };
                                    
                                    // Kanal üzerinden gRPC stream'ine gönder.
                                    // Eğer gönderim başarısız olursa (istemci bağlantıyı kapattıysa),
                                    // artık göndermeye çalışmamak için sender'ı None yap.
                                    if sender.send(Ok(audio_frame)).await.is_err() {
                                        info!("gRPC stream alıcısı kapandı, canlı ses akışı durduruldu.");
                                        live_stream_sender = None;
                                    }
                                },
                                Err(e) => error!(error = %e, "Ses verisi işlenemedi."),
                            }
                        }

                        // Kalıcı kayıt mantığı aynı kalıyor, etkilenmiyor.
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

async fn finalize_and_save_recording(session: RecordingSession, config: Arc<AppConfig>) {
    let result: Result<(), anyhow::Error> = async {
        info!(uri = %session.output_uri, samples_count = session.samples.len(), "Kayıt sonlandırılıyor ve kaydediliyor...");

        if session.samples.is_empty() {
            warn!("Kaydedilecek ses verisi yok, boş dosya oluşturulmayacak.");
            return Ok(());
        }
        
        let wav_data = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, hound::Error> {
            // DÜZELTME: Vec<new>() -> Vec::new()
            let mut buffer = Cursor::new(Vec::new());
            let mut writer = WavWriter::new(&mut buffer, session.spec)?;
            for sample in session.samples {
                writer.write_sample(sample)?;
            }
            writer.finalize()?;
            Ok(buffer.into_inner())
        }).await??;

        let writer = writers::from_uri(&session.output_uri, &config).await?;
        writer.write(wav_data).await?;
        
        Ok(())
    }.await;

    if let Err(e) = result {
        error!(error = ?e, "Kayıt kaydetme görevi başarısız oldu.");
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