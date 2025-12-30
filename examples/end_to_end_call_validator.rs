// examples/end_to_end_call_validator.rs
use anyhow::Result; // DÜZELTME: Context kaldırıldı
use aws_sdk_s3::Client as S3Client;
use hound::WavReader;
use rand::Rng;
use sentiric_contracts::sentiric::media::v1::{
    media_service_client::MediaServiceClient, AllocatePortRequest, PlayAudioRequest,
    RecordAudioRequest, ReleasePortRequest, StartRecordingRequest, StopRecordingRequest,
};
use std::env;
use std::io::Cursor;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::time::sleep;
use tokio_stream::StreamExt;
use tonic::transport::Channel;

// Paylaşılan modülleri kullan
mod shared;
use shared::{grpc_client::connect_to_media_service, rtp_utils::send_pcmu_rtp_stream, s3_client::connect_to_s3};

#[tokio::main]
async fn main() -> Result<()> {

    println!("--- Uçtan Uca Medya Servisi TEMEL Doğrulama Testi Başlatılıyor ---");
    println!("---  Senaryo: PCMU kodek ile çağrı, 8kHz WAV olarak kayıt ve birleştirme ---");

    let env_file = env::var("ENV_FILE").unwrap_or_else(|_| ".env.example".to_string());
    dotenvy::from_filename(&env_file).ok();

    let mut client = connect_to_media_service().await?;
    let s3_client = connect_to_s3().await?;

    println!("\n[ADIM 1] Port alınıyor ve kayıt başlatılıyor...");
    let call_id = format!("validation-call-{}", rand::thread_rng().gen::<u32>());
    
    let allocate_res = client.allocate_port(AllocatePortRequest { call_id: call_id.clone() }).await?.into_inner();
    let rtp_port = allocate_res.rtp_port;
    
    // MEDIA_SERVICE_RECORD_BASE_PATH, .env'den "/sentiric-media-record" olarak gelir.
    let base_path = env::var("MEDIA_SERVICE_RECORD_BASE_PATH")?;

    let s3_bucket = env::var("BUCKET_NAME")?;

    // S3 key'ini oluştururken baştaki '/' karakterini kaldırıyoruz.
    // Bu, sunucudaki writers.rs mantığıyla %100 uyumlu olmasını sağlar.
    let s3_key_prefix = base_path.trim_start_matches('/');

    let s3_key = format!("{}/validation_{}.wav", s3_key_prefix, rtp_port);

    let output_uri = format!("s3://{}/{}", s3_bucket, s3_key);

    client.start_recording(StartRecordingRequest {
        server_rtp_port: rtp_port, output_uri: output_uri.clone(),
        sample_rate: None, format: None, 
        call_id, trace_id: format!("trace-{}", rand::thread_rng().gen::<u32>()),
    }).await?;
    println!("- Kayıt başlatıldı. Hedef: {}", output_uri);

    println!("\n[ADIM 2] Eş zamanlı medya akışları simüle ediliyor...");
    // rtp_target_ip değişkeni artık burada okunmuyor, rtp_utils içinde okunacak.
    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    let local_rtp_addr = socket.local_addr()?;
    println!("- [İSTEMCİ] Anonslar bu adrese beklenecek: {}", local_rtp_addr);
    
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    let mut stt_client = client.clone();
    let stt_sim_handle = tokio::spawn(async move {
        listen_to_live_audio(&mut stt_client, rtp_port, done_rx).await
    });
    
    sleep(Duration::from_millis(200)).await;

    // --- DEĞİŞİKLİK BURADA (DOĞRU HALİ) ---
    // Fonksiyon artık host IP'sini argüman olarak almıyor.
    // YENİ DOĞRU HALİ (rtp_target_ip parametresi yok):
    let user_sim_handle = tokio::spawn(
        send_pcmu_rtp_stream(rtp_port as u16, Duration::from_secs(4), 440.0)
    );
    // ------------------------------------
    
    sleep(Duration::from_millis(500)).await;
    println!("- [BOT SIM] 'welcome.wav' anonsu çalınıyor...");
    
    client.play_audio(PlayAudioRequest {
        audio_uri: "file://audio/tr/welcome.wav".to_string(),
        server_rtp_port: rtp_port, rtp_target_addr: local_rtp_addr.to_string(),
    }).await?;
    
    // RTP gönderiminin bitmesini bekle
    user_sim_handle.await??;

    // --- DEĞİŞİKLİK BURADA ---
    // Sabit bir süre beklemek yerine, STT akışının kapanmasını bekleyelim.
    // listen_to_live_audio fonksiyonu, veri akışı durduktan bir süre sonra doğal olarak sonlanacak.
    // Bu yüzden burada uzun bir bekleme ekleyerek ona zaman tanıyoruz.
    println!("- (Tüm ses akışının sunucudan geri dönmesi için bekleniyor...)");
    sleep(Duration::from_secs(5)).await; // ESKİ DEĞER: 4 saniye -> YENİ DEĞER: 5 saniye
    // -------------------------

    let _ = done_tx.send(()); 
    let received_audio_len = stt_sim_handle.await??;

    println!("- [STT SIM] {} byte temiz 16kHz LPCM ses verisi (sadece inbound) alındı.", received_audio_len);
    assert!(received_audio_len >= 115_200, "STT servisi yeterli ses verisi alamadı! (Beklenen >= 115200, Alınan: {})", received_audio_len);

    println!("\n[ADIM 3] Kayıt durduruluyor ve kaynaklar serbest bırakılıyor...");
    client.stop_recording(StopRecordingRequest { server_rtp_port: rtp_port }).await?;
    client.release_port(ReleasePortRequest { rtp_port }).await?;

    println!("\n[ADIM 4] Kayıt dosyası S3'ten indirilip doğrulanılıyor...");
    // --- DEĞİŞİKLİK BURADA ---
    // Zamanlama sorunlarını ekarte etmek için bekleme süresini önemli ölçüde artıralım.
    // CI/CD ortamları yavaş olabilir ve S3'ün tutarlılığı zaman alabilir.
    // println!("- (S3 tutarlılığı ve dosyanın yazılması için 10 saniye bekleniyor...)");
    // sleep(Duration::from_secs(10)).await; // ESKİ DEĞER: 3 saniye -> YENİ DEĞER: 10 saniye
    // -------------------------
    // Dosyayı indirirken de aynı s3_key'i kullanırız.
    let wav_data = download_from_s3(&s3_client, &s3_bucket, &s3_key).await?;
    let reader = WavReader::new(Cursor::new(wav_data))?;
    let spec = reader.spec();
    let duration = reader.duration() as f32 / spec.sample_rate as f32;
    
    println!("\n--- WAV Dosyası Analizi ---");
    println!("  - Süre: {:.2} saniye", duration);
    println!("  - Örnekleme Hızı: {} Hz", spec.sample_rate);
    assert_eq!(spec.sample_rate, 8000, "HATA: Kayıt örnekleme oranı 8kHz olmalı!");
    assert!(duration > 4.0, "HATA: Kayıt süresi çok kısa ({:.2}s)! Sesler birleştirilemedi!", duration);

    println!("\n\n✅✅✅ TEMEL DOĞRULAMA BAŞARILI ✅✅✅");
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

async fn download_from_s3(client: &S3Client, bucket: &str, key: &str) -> Result<Vec<u8>> {
    const MAX_RETRIES: u32 = 6;
    const RETRY_DELAY_SECONDS: u64 = 4;

    for attempt in 1..=MAX_RETRIES {
        println!("- (S3'ten indirme denemesi {}/{}: bucket='{}', key='{}')", attempt, MAX_RETRIES, bucket, key);
        match client.get_object().bucket(bucket).key(key).send().await {
            Ok(resp) => {
                let data = resp.body.collect().await?.into_bytes().to_vec();
                println!("- Dosya başarıyla indirildi.");
                return Ok(data);
            }
            Err(e) => {
                if attempt == MAX_RETRIES {
                    return Err(anyhow::anyhow!(e).context(format!("Dosya S3'te bulunamadı (bucket: {}, key: {})", bucket, key)));
                }
                println!("- İndirme hatası, {} saniye sonra tekrar denenecek... Hata: {}", RETRY_DELAY_SECONDS, e);
                sleep(Duration::from_secs(RETRY_DELAY_SECONDS)).await;
            }
        }
    }
    unreachable!();
}