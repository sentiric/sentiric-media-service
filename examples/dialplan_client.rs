// examples/dialplan_client.rs

use anyhow::Result;
use std::env;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};

// Gerekli tipleri import et
use sentiric_contracts::sentiric::media::v1::{
    media_service_client::MediaServiceClient,
    AllocatePortRequest,
    PlayAudioRequest, // <-- PlayAudio kullanacağız
    ReleasePortRequest,
};

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    println!("--- Dialplan Service İstemci Simülasyonu Başlatılıyor ---");
    println!("--- SENARYO: Arayana IVR menüsü çalınacak. ---");

    // 1. Sertifika yollarını dialplan-service için güncelle
    let client_cert_path = env::var("DIALPLAN_SERVICE_CERT_PATH")?;
    let client_key_path = env::var("DIALPLAN_SERVICE_KEY_PATH")?;
    let ca_path = env::var("GRPC_TLS_CA_PATH")?;
    
    // Bağlanılacak Media Service adresi
    let media_service_host = env::var("MEDIA_SERVICE_HOST")?;
    let media_service_url = env::var("MEDIA_SERVICE_GRPC_URL")?;
    let server_addr = format!("https://{}", media_service_url);

    println!("\nMedia Service Adresi: {}", server_addr);
    println!("Kullanılacak İstemci Sertifikası: {}", client_cert_path);

    // 2. TLS ve Kanal kurulumu (her istemcide aynı)
    let client_identity = Identity::from_pem(tokio::fs::read(&client_cert_path).await?, tokio::fs::read(&client_key_path).await?);
    let server_ca_certificate = Certificate::from_pem(tokio::fs::read(&ca_path).await?);
    let tls_config = ClientTlsConfig::new()
        // 'domain_name' sertifikanın Common Name (CN) veya Subject Alternative Name (SAN)
        // alanıyla eşleşmelidir. Genellikle development'da servis adı kullanılır.
        .domain_name(media_service_host)
        .ca_certificate(server_ca_certificate)
        .identity(client_identity);

    println!("\nMedia Service'e bağlanılıyor...");
    let channel = Channel::from_shared(server_addr)?.tls_config(tls_config)?.connect().await?;
    println!("✅ Bağlantı başarılı!");
    let mut client = MediaServiceClient::new(channel);

    // --- Adım 1: Anons çalmak için port al ---
    println!("\n'AllocatePort' isteği gönderiliyor...");
    let allocate_response = client.allocate_port(tonic::Request::new(AllocatePortRequest {
        call_id: format!("dialplan-call-{}", rand::random::<u32>()),
    })).await?;
    let allocated_port = allocate_response.into_inner().rtp_port;
    println!("✅ Port tahsis edildi: {}", allocated_port);

    // --- Adım 2: Alınan porttan anonsu çal ---
    println!("\n'PlayAudio' isteği gönderiliyor (Port: {})...", allocated_port);
    // Not: Bu ses dosyasının (welcome.wav) assets klasöründe olduğundan emin olun!
    let audio_id_to_play = "audio/tr/welcome.wav"; // .wav uzantısı olmadan
    let play_audio_request = tonic::Request::new(PlayAudioRequest {
        audio_id: audio_id_to_play.to_string(),
        server_rtp_port: allocated_port,
        // Gerçek senaryoda bu, arayanın SIP telefonunun RTP alıcı adresidir.
        // Test için yerel bir porta gönderiyormuş gibi yapalım.
        rtp_target_addr: "127.0.0.1:30000".to_string(), 
    });
    let play_audio_response = client.play_audio(play_audio_request).await?;
    println!("✅ Anons çalma isteği sıraya alındı: success={}", play_audio_response.into_inner().success);
    
    // Anonsun bitmesini bekliyormuş gibi yapalım
    println!("(Anonsun çalınması için 3 saniye bekleniyor...)");
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // --- Adım 3: İş bitince portu hemen serbest bırak ---
    println!("\n'ReleasePort' isteği gönderiliyor (Port: {})...", allocated_port);
    client.release_port(tonic::Request::new(ReleasePortRequest {
        rtp_port: allocated_port,
    })).await?;
    println!("✅ Port serbest bırakıldı.");

    println!("\n--- Simülasyon Tamamlandı ---");
    Ok(())
}