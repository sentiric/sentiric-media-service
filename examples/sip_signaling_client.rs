// examples/sip_signaling_client.rs

use anyhow::Result;
use std::env;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};

// Gerekli tipleri import et
use sentiric_contracts::sentiric::media::v1::{
    media_service_client::MediaServiceClient,
    AllocatePortRequest,
    ReleasePortRequest,
};

#[tokio::main]
async fn main() -> Result<()> {
    // DEĞİŞİKLİK: .ok() yerine, sonucu kontrol edip hata varsa panic'e zorluyoruz.
    match dotenvy::from_filename("development.env") {
        Ok(_) => println!("'development.env' dosyası istemci için başarıyla yüklendi."),
        Err(e) => {
            // Test istemcisi için panic! daha uygundur, çünkü loglama altyapısı kurulmamış olabilir.
            panic!("'development.env' dosyası yüklenemedi: {}", e);
        }
    };
    
    println!("--- SIP Signaling Service İstemci Simülasyonu Başlatılıyor ---");
    println!("--- SENARYO: Media Service'in kapasitesi kontrol edilecek. ---");

    // 1. Sertifika yollarını sip-signaling-service için güncelle
    let client_cert_path = env::var("SIP_SIGNALING_SERVICE_CERT_PATH")?;
    let client_key_path = env::var("SIP_SIGNALING_SERVICE_KEY_PATH")?;
    let ca_path = env::var("GRPC_TLS_CA_PATH")?;
    
    // Bağlanılacak Media Service adresi
    let media_service_host = env::var("MEDIA_SERVICE_HOST")?;
    let media_service_url = env::var("MEDIA_SERVICE_GRPC_URL")?;
    let server_addr = format!("https://{}", media_service_url);

    println!("\nMedia Service Adresi: {}", server_addr);
    println!("Kullanılacak İstemci Sertifikası: {}", client_cert_path);

    // 2. TLS ve Kanal kurulumu
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

    // --- Adım 1: Kapasiteyi test etmek için bir port talep et ---
    println!("\n'AllocatePort' isteği gönderiliyor (kapasite kontrolü)...");
    let allocate_response = client.allocate_port(tonic::Request::new(AllocatePortRequest {
        call_id: format!("sip-healthcheck-{}", rand::random::<u32>()),
    })).await?;
    let allocated_port = allocate_response.into_inner().rtp_port;
    println!("✅ Port başarıyla tahsis edildi: {}. Media Service kapasitesi var.", allocated_port);
    
    // --- Adım 2: Portu kullanmayacağımız için hemen serbest bırak ---
    println!("\n'ReleasePort' isteği gönderiliyor (portu iade et)...");
    client.release_port(tonic::Request::new(ReleasePortRequest {
        rtp_port: allocated_port,
    })).await?;
    println!("✅ Port başarıyla iade edildi.");

    println!("\n--- Simülasyon Tamamlandı ---");
    Ok(())
}