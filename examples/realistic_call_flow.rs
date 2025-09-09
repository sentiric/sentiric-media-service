use anyhow::{Context, Result};
use aws_sdk_s3::Client as S3Client;
use base64::{engine::general_purpose, Engine};
use hound::{WavReader, WavSpec, SampleFormat};
use rand::Rng;
use sentiric_contracts::sentiric::media::v1::{
    media_service_client::MediaServiceClient, AllocatePortRequest, PlayAudioRequest,
    RecordAudioRequest, ReleasePortRequest, StartRecordingRequest, StopRecordingRequest,
};
use std::env;
use std::f32::consts::PI;
use std::io::Cursor;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::time::sleep;
use tokio_stream::StreamExt;
use tonic::transport::Channel;

mod shared;
use shared::{grpc_client::connect_to_media_service, rtp_utils::send_pcmu_rtp_stream, s3_client::connect_to_s3};

fn generate_mock_tts_wav_data() -> Result<Vec<u8>> {
    let spec = WavSpec {
        channels: 1, sample_rate: 16000,
        bits_per_sample: 16, sample_format: SampleFormat::Int,
    };
    let mut buffer = Cursor::new(Vec::new());
    let mut writer = hound::WavWriter::new(&mut buffer, spec)?;
    for t in (0..16000).map(|x| x as f32 / 16000.0) {
        let sample = (t * 440.0 * 2.0 * PI).sin();
        let amplitude = i16::MAX as f32;
        writer.write_sample((sample * amplitude) as i16)?;
    }
    writer.finalize()?;
    Ok(buffer.into_inner())
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("\n--- GERÇEKÇİ ÇAĞRI AKIŞI DOĞRULAMA TESTİ ---");
    println!("--- Senaryo: Sıralı anonslar + Eş zamanlı kullanıcı sesi. Cızırtı ve kesilme hatalarını doğrular. ---");

    let env_file = env::var("ENV_FILE").unwrap_or_else(|_| ".env.test".to_string());
    dotenvy::from_filename(&env_file).ok();

    let mut client = connect_to_media_service().await?;
    let s3_client = connect_to_s3().await?;

    println!("\n[ADIM 1] Port alınıyor ve kalıcı kayıt başlatılıyor...");
    let call_id = format!("realistic-flow-{}", rand::thread_rng().gen::<u32>());
    let allocate_res = client.allocate_port(AllocatePortRequest { call_id: call_id.clone() }).await?.into_inner();
    let rtp_port = allocate_res.rtp_port;

    let s3_bucket = env::var("S3_BUCKET_NAME")?;
    let s3_key = format!("test/realistic_flow_{}.wav", rtp_port);
    let output_uri = format!("s3://{}/{}", s3_bucket, s3_key);

    client.start_recording(StartRecordingRequest {
        server_rtp_port: rtp_port, output_uri: output_uri.clone(),
        sample_rate: None, format: None,
        call_id, trace_id: "trace-realistic-flow".to_string(),
    }).await?;
    println!("- Kalıcı kayıt başlatıldı. Hedef: {}", output_uri);

    println!("\n[ADIM 2] Eş zamanlı medya akışları simüle ediliyor...");
    // rtp_target_ip değişkeni artık burada okunmuyor.
    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    let local_rtp_addr = socket.local_addr()?;
    println!("- [İSTEMCİ] Anonslar bu adrese beklenecek: {}", local_rtp_addr);

    let (stt_done_tx, stt_done_rx) = tokio::sync::oneshot::channel();
    let mut stt_client = client.clone();
    let stt_sim_handle = tokio::spawn(async move {
        listen_to_live_audio(&mut stt_client, rtp_port, stt_done_rx).await
    });

    // --- DEĞİŞİKLİK BURADA (DOĞRU HALİ) ---
    // Fonksiyon artık host IP'sini argüman olarak almıyor.
    let user_sim_handle = tokio::spawn(
        send_pcmu_rtp_stream(rtp_port as u16, Duration::from_secs(4), 440.0)
    );
    // ------------------------------------
    
    println!("- [SİSTEM] 'connecting.wav' anonsu gönderiliyor...");
    
    client.play_audio(PlayAudioRequest {
        audio_uri: "file://audio/tr/system/connecting.wav".to_string(),
        server_rtp_port: rtp_port, rtp_target_addr: local_rtp_addr.to_string(),
    }).await?;

    sleep(Duration::from_millis(100)).await;
    let mock_tts_data = generate_mock_tts_wav_data()?;
    let tts_base64 = general_purpose::STANDARD.encode(&mock_tts_data);
    let tts_data_uri = format!("data:audio/wav;base64,{}", tts_base64);
    
    println!("- [SİSTEM] Simüle edilmiş TTS yanıtı (1sn) anında gönderiliyor (kuyruğa alınmalı)...");
    client.play_audio(PlayAudioRequest {
        audio_uri: tts_data_uri,
        server_rtp_port: rtp_port, rtp_target_addr: local_rtp_addr.to_string(),
    }).await?;
    
    // --- DEĞİŞİKLİK BURADA ---
    // Bu testte birden fazla anons ve daha karmaşık bir akış var.
    // S3'e yazma ve tutarlılık için daha da fazla zaman tanıyalım.
    println!("- (Tüm anonsların bitmesi ve ses akışının işlenmesi için bekleniyor...)");
    sleep(Duration::from_secs(10)).await; // ESKİ DEĞER: 6 saniye -> YENİ DEĞER: 10 saniye
    // -------------------------

    let _ = stt_done_tx.send(());
    let stt_total_bytes = stt_sim_handle.await??;

    println!("- [STT SİM] {} byte temiz 16kHz LPCM verisi aldı.", stt_total_bytes);
    assert!(stt_total_bytes > 115_200, "HATA: Gelen ses verisi (STT için) beklenenden çok az. Beklenen > 115200, Alınan: {}. Cızırtı/kayıp var!", stt_total_bytes);

    println!("\n[ADIM 3] Kayıt durduruluyor ve kaynaklar serbest bırakılıyor...");
    client.stop_recording(StopRecordingRequest { server_rtp_port: rtp_port }).await?;
    client.release_port(ReleasePortRequest { rtp_port }).await?;

    println!("\n[ADIM 4] Sonuç kaydı S3'ten doğrulanıyor...");
    // --- DEĞİŞİKLİK BURADA ---
    // Tıpkı diğer testteki gibi, S3 indirmesinden önce de sağlam bir bekleme ekleyelim.
    // println!("- (S3 tutarlılığı için 10 saniye bekleniyor...)");
    // sleep(Duration::from_secs(10)).await; // YENİ EKLENDİ
    // -------------------------
    let wav_data = download_from_s3(&s3_client, &s3_bucket, &s3_key).await?;
    let reader = WavReader::new(Cursor::new(wav_data))?;
    let spec = reader.spec();
    let duration = reader.duration() as f32 / spec.sample_rate as f32;

    println!("\n--- KAYIT ANALİZİ ---");
    println!("  - Süre: {:.2} saniye", duration);
    println!("  - Örnekleme Oranı: {} Hz", spec.sample_rate);
    
    let expected_duration = 4.0;
    assert_eq!(spec.sample_rate, 8000, "HATA: Kayıt 8kHz olmalı!");
    assert!(duration > expected_duration, "HATA: Kayıt süresi çok kısa ({:.2}s)! Anonslar kesildi veya sesler birleştirilemedi! Beklenen > {}", duration, expected_duration);

    println!("\n✅✅✅ REALISTIC FLOW TEST BAŞARILI ✅✅✅");
    Ok(())
}

async fn listen_to_live_audio(client: &mut MediaServiceClient<Channel>, port: u32, mut done_rx: tokio::sync::oneshot::Receiver<()>) -> Result<usize> {
    let mut stream = client.record_audio(RecordAudioRequest{server_rtp_port:port,target_sample_rate:Some(16000)}).await?.into_inner();
    let mut total_bytes = 0;
    loop {
        tokio::select!{
            biased;
            _ = &mut done_rx => { break; },
            maybe_item = tokio::time::timeout(Duration::from_secs(3), stream.next()) => {
                match maybe_item {
                    Ok(Some(Ok(res))) => { total_bytes += res.audio_data.len(); },
                    Ok(Some(Err(e))) => { eprintln!("Stream hatası: {}", e); break; },
                    Ok(None) => break,
                    Err(_) => {
                        println!("- [STT DINLEYICI] 3 saniyedir yeni ses verisi gelmiyor, dinleyici kapatılıyor.");
                        break;
                    }
                }
            }
        }
    }
    Ok(total_bytes)
}

// Bu fonksiyonu her iki test dosyasında da güncelleyin:
// end_to_end_call_validator.rs ve realistic_call_flow.rs
async fn download_from_s3(client: &S3Client, bucket: &str, key: &str) -> Result<Vec<u8>> {
    const MAX_RETRIES: u32 = 5;
    const RETRY_DELAY_SECONDS: u64 = 3;

    for attempt in 1..=MAX_RETRIES {
        println!("- (S3'ten indirme denemesi {}/{})", attempt, MAX_RETRIES);
        match client.get_object().bucket(bucket).key(key).send().await {
            Ok(resp) => {
                let data = resp.body.collect().await?.into_bytes().to_vec();
                println!("- Dosya başarıyla indirildi.");
                return Ok(data);
            }
            Err(e) => {
                if attempt == MAX_RETRIES {
                    // Son denemede de başarısız olursa hatayı döndür
                    return Err(e.into());
                }
                println!("- Dosya bulunamadı veya bir hata oluştu, {} saniye sonra tekrar denenecek...", RETRY_DELAY_SECONDS);
                sleep(Duration::from_secs(RETRY_DELAY_SECONDS)).await;
            }
        }
    }
    unreachable!(); // Bu satıra asla ulaşılmamalı
}