// File: sentiric-media-service/src/rtp/session.rs
use std::net::SocketAddr;
use std::sync::Arc;
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

// PCMU (G.711 μ-law) to 16-bit PCM dönüşüm tablosu - DOĞRU VERSİYON
const ULAW_TO_PCM: [i16; 256] = [
    -32124, -31100, -30076, -29052, -28028, -27004, -25980, -24956,
    -23932, -22908, -21884, -20860, -19836, -18812, -17788, -16764,
    -15996, -15484, -14972, -14460, -13948, -13436, -12924, -12412,
    -11900, -11388, -10876, -10364,  -9852,  -9340,  -8828,  -8316,
     -7932,  -7676,  -7420,  -7164,  -6908,  -6652,  -6396,  -6140,
     -5884,  -5628,  -5372,  -5116,  -4860,  -4604,  -4348,  -4092,
     -3900,  -3772,  -3644,  -3516,  -3388,  -3260,  -3132,  -3004,
     -2876,  -2748,  -2620,  -2492,  -2364,  -2236,  -2108,  -1980,
     -1884,  -1820,  -1756,  -1692,  -1628,  -1564,  -1500,  -1436,
     -1372,  -1308,  -1244,  -1180,  -1116,  -1052,   -988,   -924,
      -876,   -844,   -812,    -780,   -748,   -716,   -684,   -652,
      -620,   -588,   -556,   -524,   -492,   -460,   -428,   -396,
      -372,   -356,   -340,    -324,   -308,   -292,   -276,   -260,
      -244,   -228,   -212,   -196,   -180,   -164,   -148,   -132,
      -120,   -112,   -104,     -96,     -88,     -80,     -72,     -64,
       -56,    -48,    -40,    -32,    -24,    -16,     -8,      0,
     32124,  31100,  30076,  29052,  28028,  27004,  25980,  24956,
     23932,  22908,  21884,  20860,  19836,  18812,  17788,  16764,
     15996,  15484,  14972,  14460,  13948,  13436,  12924,  12412,
     11900,  11388,  10876,  10364,   9852,   9340,   8828,   8316,
      7932,   7676,   7420,   7164,   6908,   6652,   6396,   6140,
      5884,   5628,   5372,   5116,   4860,   4604,   4348,   4092,
      3900,   3772,   3644,   3516,   3388,   3260,   3132,   3004,
      2876,   2748,   2620,   2492,   2364,   2236,   2108,   1980,
      1884,   1820,   1756,   1692,   1628,   1564,   1500,   1436,
      1372,   1308,   1244,   1180,   1116,   1052,    988,    924,
       876,    844,    812,    780,    748,    716,    684,    652,
       620,    588,    556,    524,    492,    460,    428,    396,
       372,    356,    340,    324,    308,    292,    276,    260,
       244,    228,    212,    196,    180,    164,    148,    132,
       120,    112,    104,     96,     88,     80,     72,     64,
        56,     48,     40,     32,     24,     16,      8,      0
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
    
    // WAV dosyasını oluştur
    let wav_filename = format!("/tmp/call_dump_port_{}.wav", port);
    let spec = WavSpec {
        channels: 1,
        sample_rate: 16000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    
    let writer = match WavWriter::create(&wav_filename, spec) {
        Ok(writer) => {
            info!(filename = %wav_filename, "Debug: Gelen ve işlenen ses bu dosyaya kaydedilecek.");
            Some(writer)
        }
        Err(e) => {
            error!(error = %e, "WAV dosyası oluşturulamadı");
            None
        }
    };
    
    let wav_writer = Arc::new(Mutex::new(writer));
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
                            
                            let audio_frame_result = process_audio_chunk(pcmu_payload, *target_rate_opt);
                            
                            match audio_frame_result {
                                Ok((pcm_bytes, pcm_samples_i16)) => {
                                    // WAV dosyasına yaz
                                    let wav_writer_clone = wav_writer.clone();
                                    let samples = pcm_samples_i16.clone();
                                    tokio::spawn(async move {
                                        let mut writer_guard = wav_writer_clone.lock().await;
                                        if let Some(writer) = writer_guard.as_mut() {
                                            for sample in &samples {
                                                if let Err(e) = writer.write_sample(*sample) {
                                                    error!(error = %e, "WAV yazma hatası");
                                                    break;
                                                }
                                            }
                                        }
                                    });

                                    let audio_frame = AudioFrame {
                                        data: pcm_bytes,
                                        media_type: if target_rate_opt.is_some() {
                                            format!("audio/l16;rate={}", target_rate_opt.unwrap())
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
    
    // WAV dosyasını düzgünce kapat
    let mut writer_guard = wav_writer.lock().await;
    if let Some(writer) = writer_guard.take() {
        match writer.finalize() {
            Ok(_) => info!(filename = %wav_filename, "Debug: Ses kaydı tamamlandı ve dosya kapatıldı."),
            Err(e) => error!(error = %e, "Debug WAV dosyası finalize edilemedi."),
        }
    }
    
    info!("RTP oturumu temizleniyor...");
    port_manager.remove_session(port).await;
    port_manager.quarantine_port(port).await;
}

fn process_audio_chunk(pcmu_payload: &[u8], target_sample_rate: Option<u32>) -> Result<(Bytes, Vec<i16>), anyhow::Error> {
    const SOURCE_SAMPLE_RATE: u32 = 8000;
    
    let pcm_samples_i16: Vec<i16> = pcmu_payload.iter()
        .map(|&byte| ULAW_TO_PCM[byte as usize])
        .collect();
    
    // Eğer target sample rate belirtilmemişse veya kaynakla aynıysa, dönüşüm yapma
    let target_rate = target_sample_rate.unwrap_or(SOURCE_SAMPLE_RATE);
    if target_rate == SOURCE_SAMPLE_RATE {
        let mut bytes = Vec::with_capacity(pcm_samples_i16.len() * 2);
        for &sample in &pcm_samples_i16 {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        return Ok((Bytes::from(bytes), pcm_samples_i16));
    }

    // PCM to F32 conversion
    let pcm_f32: Vec<f32> = pcm_samples_i16.iter()
        .map(|&sample| sample as f32 / 32768.0)
        .collect();

    // Resampling
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
    
    // F32 to PCM conversion
    let resampled_i16: Vec<i16> = resampled_f32.into_iter()
        .map(|s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16)
        .collect();

    let mut bytes = Vec::with_capacity(resampled_i16.len() * 2);
    for &sample in &resampled_i16 {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    
    Ok((Bytes::from(bytes), resampled_i16))
}