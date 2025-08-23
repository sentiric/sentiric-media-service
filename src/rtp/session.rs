// File: sentiric-media-service/src/rtp/session.rs
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tonic::Status;
use tracing::{info, instrument, warn, error};
use bytes::Bytes;

use crate::audio::AudioCache;
use crate::config::AppConfig;
use crate::rtp::command::{RtpCommand, AudioFrame};
use crate::rtp::stream::send_announcement_from_uri;
use crate::state::PortManager;
use hound::{WavSpec, WavWriter};
use rubato::{Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};
use std::io::Cursor;

// PCMU (G.711 μ-law) to 16-bit PCM dönüşüm tablosu
const ULAW_TO_PCM: [i16; 256] = [
    -32124, -31100, -30076, -29052, -28028, -27004, -25980, -24956, -23932, -22908, -21884, -20860, -19836, -18812, -17788, -16764,
    -15996, -15484, -14972, -14460, -13948, -13436, -12924, -12412, -11900, -11388, -10876, -10364, -9852, -9340, -8828, -8316,
    -7932, -7676, -7420, -7164, -6908, -6652, -6396, -6140, -5884, -5628, -5372, -5116, -4860, -4604, -4348, -4092,
    -3898, -3770, -3642, -3514, -3386, -3258, -3130, -3002, -2874, -2746, -2618, -2490, -2362, -2234, -2106, -1978,
    -1884, -1820, -1756, -1692, -1628, -1564, -1500, -1436, -1372, -1308, -1244, -1180, -1116, -1052, -988, -924,
    -870, -838, -806, -774, -742, -710, -678, -646, -614, -582, -550, -518, -486, -454, -422, -390,
    -364, -348, -332, -316, -300, -284, -268, -252, -236, -220, -204, -188, -172, -156, -140, -124,
    -110, -102, -94, -86, -78, -70, -62, -54, -46, -38, -30, -22, -14, -6, 2, 10,
    32124, 31100, 30076, 29052, 28028, 27004, 25980, 24956, 23932, 22908, 21884, 20860, 19836, 18812, 17788, 16764,
    15996, 15484, 14972, 14460, 13948, 13436, 12924, 12412, 11900, 11388, 10876, 10364, 9852, 9340, 8828, 8316,
    7932, 7676, 7420, 7164, 6908, 6652, 6396, 6140, 5884, 5628, 5372, 5116, 4860, 4604, 4348, 4092,
    3898, 3770, 3642, 3514, 3386, 3258, 3130, 3002, 2874, 2746, 2618, 2490, 2362, 2234, 2106, 1978,
    1884, 1820, 1756, 1692, 1628, 1564, 1500, 1436, 1372, 1308, 1244, 1180, 1116, 1052, 988, 924,
    870, 838, 806, 774, 742, 710, 678, 646, 614, 582, 550, 518, 486, 454, 422, 390,
    364, 348, 332, 316, 300, 284, 268, 252, 236, 220, 204, 188, 172, 156, 140, 124,
    110, 102, 94, 86, 78, 70, 62, 54, -46, -38, -30, -22, -14, -6, -2, -10
];

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
                        if let Some(token) = current_playback_token.take() { info!("Devam eden bir anons var, iptal ediliyor."); token.cancel(); }
                        current_playback_token = Some(cancellation_token.clone());
                        let target = actual_remote_addr.unwrap_or(candidate_target_addr);
                        tokio::spawn(send_announcement_from_uri(socket.clone(), target, audio_uri, audio_cache.clone(), config.clone(), cancellation_token));
                    },
                    RtpCommand::StartRecording { stream_sender, target_sample_rate } => {
                        info!(target_rate = ?target_sample_rate, "Ses kaydı komutu alındı.");
                        recording_sender = Some((stream_sender, target_sample_rate));
                    },
                    RtpCommand::StopAudio => {
                        if let Some(token) = current_playback_token.take() { info!("Anons çalma komutu dışarıdan durduruldu."); token.cancel(); }
                    },
                    RtpCommand::Shutdown => {
                        info!("Shutdown komutu alındı, oturum sonlandırılıyor.");
                        if let Some(token) = current_playback_token.take() { token.cancel(); }
                        if let Some((sender, _)) = recording_sender.take() { drop(sender); }
                        break;
                    }
                }
            },
            result = socket.recv_from(&mut buf) => {
                if let Ok((len, addr)) = result {
                    if len > 12 {
                        if actual_remote_addr.is_none() { info!(remote = %addr, "İlk RTP paketi alındı, hedef adres doğrulandı."); actual_remote_addr = Some(addr); }
                        if let Some((sender, target_rate_opt)) = &recording_sender {
                            let pcmu_payload = &buf[12..len];
                            
                            // --- DEĞİŞİKLİK BURADA BAŞLIYOR ---
                            let audio_frame = if let Some(target_rate) = target_rate_opt {
                                match process_audio_chunk(pcmu_payload, *target_rate) {
                                    Ok(pcm_bytes) => {
                                        // Artık media_type "audio/l16" (raw pcm)
                                        AudioFrame { data: pcm_bytes, media_type: format!("audio/l16;rate={}", target_rate) }
                                    },
                                    Err(e) => {
                                        error!(error = %e, "Ses verisi işlenemedi.");
                                        continue;
                                    }
                                }
                            } else {
                                // Hedef sample rate yoksa, ham PCMU gönder
                                AudioFrame { data: Bytes::copy_from_slice(pcmu_payload), media_type: "audio/pcmu".to_string() }
                            };
                            // --- DEĞİŞİKLİK BİTİYOR ---

                            if sender.send(Ok(audio_frame)).await.is_err() {
                                warn!("Kayıt stream'i istemci tarafından kapatıldı, kayıt durduruluyor.");
                                recording_sender = None;
                            }
                        }
                    }
                }
            },
        }
    }
    
    info!("RTP oturumu temizleniyor...");
    port_manager.remove_session(port).await;
    port_manager.quarantine_port(port).await;
}

fn process_audio_chunk(pcmu_payload: &[u8], target_sample_rate: u32) -> Result<Bytes, anyhow::Error> {
    const SOURCE_SAMPLE_RATE: u32 = 8000;
    
    let pcm_samples: Vec<i16> = pcmu_payload.iter().map(|&byte| ULAW_TO_PCM[byte as usize]).collect();
    let pcm_f32: Vec<f32> = pcm_samples.iter().map(|&s| s as f32 / 32768.0).collect();

    let resampled_f32 = if target_sample_rate != SOURCE_SAMPLE_RATE {
        let params = SincInterpolationParameters {
            sinc_len: 256, f_cutoff: 0.95, interpolation: SincInterpolationType::Linear,
            oversampling_factor: 256, window: WindowFunction::BlackmanHarris2,
        };
        let mut resampler = SincFixedIn::<f32>::new(
            target_sample_rate as f64 / SOURCE_SAMPLE_RATE as f64, 2.0, params,
            pcm_f32.len(), 1,
        )?;
        resampler.process(&[pcm_f32], None)?.remove(0)
    } else {
        pcm_f32
    };

    let resampled_i16: Vec<i16> = resampled_f32.into_iter().map(|s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16).collect();

    // i16 vektörünü doğrudan byte vektörüne dönüştür (Little-Endian)
    let mut bytes = Vec::with_capacity(resampled_i16.len() * 2);
    for sample in resampled_i16 {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    
    Ok(Bytes::from(bytes))
}

fn resample_and_encode_wav(pcmu_payload: &[u8], target_sample_rate: u32) -> Result<Bytes, anyhow::Error> {
    const SOURCE_SAMPLE_RATE: u32 = 8000;
    
    // 1. PCMU'dan 16-bit PCM'e dönüştür
    let pcm_samples: Vec<i16> = pcmu_payload.iter().map(|&byte| ULAW_TO_PCM[byte as usize]).collect();
    
    // 2. PCM'i f32'ye dönüştür
    let pcm_f32: Vec<f32> = pcm_samples.iter().map(|&s| s as f32 / 32768.0).collect();

    // 3. Yeniden örnekle (resample)
    let resampled_f32 = if target_sample_rate != SOURCE_SAMPLE_RATE {
        let params = SincInterpolationParameters {
            sinc_len: 256,
            f_cutoff: 0.95,
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: 256,
            window: WindowFunction::BlackmanHarris2,
        };
        let mut resampler = SincFixedIn::<f32>::new(
            target_sample_rate as f64 / SOURCE_SAMPLE_RATE as f64,
            2.0,
            params,
            pcm_f32.len(),
            1,
        )?;
        resampler.process(&[pcm_f32], None)?.remove(0)
    } else {
        pcm_f32
    };

    // 4. f32'yi tekrar i16'ya dönüştür
    let resampled_i16: Vec<i16> = resampled_f32.into_iter().map(|s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16).collect();

    // 5. WAV formatında belleğe yaz
    let spec = WavSpec {
        channels: 1,
        sample_rate: target_sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut cursor = Cursor::new(Vec::new());
    // DİKKAT: hound'un WavWriter'ı doğal olarak Little-Endian yazar, bu yüzden ek bir işleme gerek yok.
    // Ancak, emin olmak için bunu açıkça yapabiliriz.
    let mut writer = WavWriter::new(&mut cursor, spec)?;
    for sample in resampled_i16 {
        // to_le_bytes() ile Little-Endian formatında olduğundan emin oluyoruz.
        writer.write_sample(sample)?;
    }
    writer.finalize()?;
    
    Ok(Bytes::from(cursor.into_inner()))
}