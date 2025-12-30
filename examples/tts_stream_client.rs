// examples/tts_stream_client.rs

use anyhow::Result;
use rand::Rng;
use std::env;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::time::sleep;
use tokio_stream::StreamExt;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};
use sentiric_contracts::sentiric::media::v1::{
    media_service_client::MediaServiceClient, AllocatePortRequest, ReleasePortRequest,
    StreamAudioToCallRequest,
};

// 16kHz'lik bir sinüs dalgası üreten fonksiyon (Chunk Generator)
fn generate_audio_chunk(duration_ms: u64, frequency: f32) -> Vec<u8> {
    let sample_rate = 16000.0;
    let num_samples = (sample_rate * (duration_ms as f32 / 1000.0)) as usize;
    let mut buffer = Vec::with_capacity(num_samples * 2);
    
    for i in 0..num_samples {
        let t = i as f32 / sample_rate;
        let sample = (t * frequency * 2.0 * std::f32::consts::PI).sin();
        let int_sample = (sample * i16::MAX as f32) as i16;
        buffer.extend_from_slice(&int_sample.to_le_bytes());
    }
    buffer
}

#[tokio::main]
async fn main() -> Result<()> {
    // .env dosyasını yükle
    let env_file = env::var("ENV_FILE").unwrap_or_else(|_| ".env.example".to_string());
    if let Err(e) = dotenvy::from_filename(&env_file) {
        println!("UYARI: .env dosyası yüklenemedi, varsayılanlar kullanılacak: {}", e);
    }

    println!("--- TTS Streaming Test İstemcisi Başlatılıyor ---");

    // 1. Bağlantı Kurulumu
    let client_cert_path = env::var("AGENT_SERVICE_CERT_PATH").expect("AGENT_SERVICE_CERT_PATH gerekli");
    let client_key_path = env::var("AGENT_SERVICE_KEY_PATH").expect("AGENT_SERVICE_KEY_PATH gerekli");
    let ca_path = env::var("GRPC_TLS_CA_PATH").expect("GRPC_TLS_CA_PATH gerekli");
    let media_host = env::var("MEDIA_SERVICE_HOST").unwrap_or_else(|_| "localhost".to_string());
    let server_addr = format!("https://{}:{}", media_host, env::var("MEDIA_SERVICE_GRPC_PORT").unwrap_or_else(|_| "13031".to_string()));

    let client_identity = Identity::from_pem(
        tokio::fs::read(&client_cert_path).await?, 
        tokio::fs::read(&client_key_path).await?
    );
    let ca_cert = Certificate::from_pem(tokio::fs::read(&ca_path).await?);
    
    let tls_config = ClientTlsConfig::new()
        .domain_name(&media_host)
        .ca_certificate(ca_cert)
        .identity(client_identity);

    let channel = Channel::from_shared(server_addr)?
        .tls_config(tls_config)?
        .connect()
        .await?;

    let mut client = MediaServiceClient::new(channel);
    println!("✅ Media Service'e bağlanıldı.");

    // 2. Port Alma (ve Call ID oluşturma)
    let call_id = format!("tts-test-{}", rand::thread_rng().gen::<u32>());
    let allocate_res = client.allocate_port(AllocatePortRequest { call_id: call_id.clone() }).await?;
    let rtp_port = allocate_res.into_inner().rtp_port;
    println!("✅ Port alındı: {} (CallID: {})", rtp_port, call_id);

    // 3. RTP Dinleyici Başlat (Gelen sesi duymak/doğrulamak için)
    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    let _local_addr = socket.local_addr()?;
    
    // Media Service'in RTP paketlerini bize göndermesi için bir 'hedef' adrese ihtiyacı var.
    // Ancak `StreamAudioToCall` metodunda hedef adres parametresi YOK (Sadece Call ID var).
    // Media Service, ancak biz ona RTP paketi atarsak hedef adresimizi öğrenir (Symmetric RTP).
    // O yüzden "Dummy" bir RTP paketi atıyoruz.
    let media_rtp_addr = format!("{}:{}", env::var("RTP_SERVICE_HOST").unwrap_or_else(|_| "127.0.0.1".to_string()), rtp_port);
    socket.send_to(&[0u8; 10], &media_rtp_addr).await?;
    println!("ℹ️ NAT delme/hedef tanıtma paketi gönderildi -> {}", media_rtp_addr);

    // 4. Stream Başlatma
    // Stream, bir iteratör veya channel üzerinden beslenir.
    let (tx, rx) = tokio::sync::mpsc::channel(10);
    let request_stream = tokio_stream::wrappers::ReceiverStream::new(rx);

    // İlk mesaj (Call ID ile)
    tx.send(StreamAudioToCallRequest {
        call_id: call_id.clone(),
        audio_chunk: vec![], // İlk mesajda data olmak zorunda değil
    }).await?;

    // Streaming işlemini ayrı task'ta başlat
    let mut response_stream = client.stream_audio_to_call(request_stream).await?.into_inner();
    
    tokio::spawn(async move {
        // Response stream'i dinle (Hata var mı?)
        while let Some(msg) = response_stream.next().await {
            match msg {
                Ok(_) => {}, // Başarılı, sessiz ol
                Err(e) => eprintln!("❌ Stream yanıt hatası: {}", e),
            }
        }
        println!("ℹ️ Stream yanıt kanalı kapandı.");
    });

    println!("📢 TTS verisi gönderiliyor (3 saniye, 440Hz)...");
    
    // Ses gönderim döngüsü
    for _ in 0..15 { // 15 chunk * 200ms = 3 saniye
        let chunk = generate_audio_chunk(200, 440.0);
        tx.send(StreamAudioToCallRequest {
            call_id: "".to_string(), // Sonraki mesajlarda call_id zorunlu değil (ama protobuf gerektiriyorsa boş atılabilir)
            audio_chunk: chunk,
        }).await?;
        sleep(Duration::from_millis(200)).await;
        print!(".");
        use std::io::Write;
        std::io::stdout().flush().unwrap();
    }
    
    println!("\n✅ Ses gönderimi tamamlandı.");
    
    // 5. Temizlik
    client.release_port(ReleasePortRequest { rtp_port }).await?;
    println!("✅ Port serbest bırakıldı.");

    Ok(())
}