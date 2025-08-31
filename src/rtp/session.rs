// File: src/rtp/session.rs (GÜNCELLENDİ)

use crate::audio::AudioCache;
use crate::config::AppConfig;
use crate::metrics::ACTIVE_SESSIONS;
use crate::rtp::command::{AudioFrame, RecordingSession, RtpCommand};
use crate::rtp::stream::{decode_audio_with_symphonia, send_announcement_from_uri};
use crate::rtp::writers;
use crate::state::PortManager;
use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose, Engine};
use hound::WavWriter;
use metrics::gauge;
use rtp::packet::Packet;
use std::io::Cursor;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tonic::Status;
use tracing::{error, info, instrument, warn};
use webrtc_util::marshal::Unmarshal;

use crate::rtp::codecs::{self, AudioCodec};

pub struct RtpSessionConfig {
    pub port_manager: PortManager,
    pub audio_cache: AudioCache,
    pub app_config: Arc<AppConfig>,
    pub port: u16,
}

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
    
    let mut outbound_codec = AudioCodec::Pcmu;

    loop {
        tokio::select! {
            biased;
            Some(command) = rx.recv() => {
                match command {
                    RtpCommand::PlayAudioUri { audio_uri, candidate_target_addr, cancellation_token } => {
                        if let Some(token) = current_playback_token.take() {
                            token.cancel();
                        }
                        current_playback_token = Some(cancellation_token.clone());
                        
                        if actual_remote_addr.is_none() {
                            info!(remote = %candidate_target_addr, "İlk paket gelmeden PlayAudio komutu alındı. Hedef adres isteğe göre ayarlandı.");
                            actual_remote_addr = Some(candidate_target_addr);
                        }
                        
                        let target = actual_remote_addr.unwrap_or(candidate_target_addr);
                        if let Some(rec_session) = &mut permanent_recording_session {
                            info!("Giden ses (outbound) kalıcı kayda ekleniyor.");
                            match load_samples_from_uri(&audio_uri, &config.audio_cache, &config.app_config).await {
                                Ok(samples_16khz) => {
                                    rec_session.samples.extend_from_slice(&samples_16khz);
                                    info!(samples_added = samples_16khz.len(), "Giden ses örnekleri kayda eklendi.");
                                },
                                Err(e) => error!(error = ?e, "Kayıt için giden ses yüklenemedi."),
                            }
                        }
                        
                        tokio::spawn(send_announcement_from_uri(
                            socket.clone(),
                            target,
                            audio_uri,
                            config.audio_cache.clone(),
                            config.app_config.clone(),
                            cancellation_token,
                            outbound_codec,
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
                        outbound_codec = AudioCodec::Pcma;
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
                        if actual_remote_addr.is_none() {
                            info!(remote = %addr, "İlk RTP paketi alındı, hedef adres doğrulandı.");
                        }
                        actual_remote_addr = Some(addr);

                        let mut packet_buf = &buf[..len];
                        let packet = match Packet::unmarshal(&mut packet_buf) {
                            Ok(p) => p,
                            Err(e) => {
                                warn!(error = %e, "Geçersiz RTP paketi alındı, atlanıyor.");
                                continue;
                            }
                        };

                        let incoming_codec = match AudioCodec::from_rtp_payload_type(packet.header.payload_type) {
                            Ok(c) => c,
                            Err(e) => {
                                warn!(error = ?e, "Desteklenmeyen codecli paket alındı, atlanıyor.");
                                continue;
                            }
                        };

                        match codecs::decode_g711_to_lpcm16(&packet.payload, incoming_codec) {
                            Ok(standardized_samples_16khz) => {
                                if let Some((sender, _)) = &live_stream_sender {
                                    let media_type = "audio/L16;rate=16000".to_string();
                                    let mut bytes = Vec::with_capacity(standardized_samples_16khz.len() * 2);
                                    for &sample in &standardized_samples_16khz {
                                        bytes.extend_from_slice(&sample.to_le_bytes());
                                    }
                                    let frame = AudioFrame { data: bytes.into(), media_type };
                                    if sender.send(Ok(frame)).await.is_err() {
                                        live_stream_sender = None;
                                    }
                                }

                                if let Some(session) = &mut permanent_recording_session {
                                    session.samples.extend_from_slice(&standardized_samples_16khz);
                                }
                            },
                            Err(e) => {
                                error!(error = %e, "Ses verisi standart formata dönüştürülemedi.");
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
            // ÖNEMLİ: WavSpec her zaman 16kHz olmalı, çünkü tüm sesler o formata çevriliyor.
            let spec_16khz = hound::WavSpec {
                channels: 1,
                sample_rate: 16000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            let mut writer = WavWriter::new(&mut buffer, spec_16khz)?;
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