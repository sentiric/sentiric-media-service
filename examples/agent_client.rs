// examples/agent_client.rs

use anyhow::Result;
use std::env; // EKLE
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};

// Artık doğrudan `sentiric_contracts`'e erişiyoruz.
use sentiric_contracts::sentiric::media::v1::{
    media_service_client::MediaServiceClient,
    AllocatePortRequest,
    ReleasePortRequest,
};

#[tokio::main]
async fn main() -> Result<()> {
    // --- DEĞİŞİKLİK: ESNEK .ENV YÜKLEME ---
    let env_file = env::var("ENV_FILE").unwrap_or_else(|_| ".env.development".to_string());
    match dotenvy::from_filename(&env_file) {
        Ok(_) => println!("'{}' dosyası istemci için başarıyla yüklendi.", env_file),
        Err(e) => {
            panic!("'{}' dosyası yüklenemedi: {}", env_file, e);
        }
    };

    println!("--- Agent Service İstemci Simülasyonu Başlatılıyor ---");

    // 1. .env dosyasından gerekli sertifika yollarını ve adresi oku
    // İstemcinin (agent-service) kimlik bilgileri
    let client_cert_path = env::var("AGENT_SERVICE_CERT_PATH")?;
    let client_key_path = env::var("AGENT_SERVICE_KEY_PATH")?;
    
    // Güven zincirinin kökü (ortak CA)
    let ca_path = env::var("GRPC_TLS_CA_PATH")?;
    
    // Bağlanılacak Media Service adresi
    let media_service_host = env::var("MEDIA_SERVICE_HOST")?;
    let media_service_url = env::var("MEDIA_SERVICE_GRPC_URL")?;
    let server_addr = format!("https://{}", media_service_url);

    println!("Media Service Adresi: {}", server_addr);
    println!("Kullanılacak İstemci Sertifikası: {}", client_cert_path);

    // 2. Sertifikaları diskten byte olarak oku
    let client_cert = tokio::fs::read(&client_cert_path).await?;
    let client_key = tokio::fs::read(&client_key_path).await?;
    let ca_cert = tokio::fs::read(&ca_path).await?;

    // 3. Tonic için kimlik (Identity) ve CA (Certificate) nesnelerini oluştur
    let client_identity = Identity::from_pem(client_cert, client_key);
    let server_ca_certificate = Certificate::from_pem(ca_cert);

    // 4. İstemci için mTLS konfigürasyonunu hazırla
    let tls_config = ClientTlsConfig::new()
        // 'domain_name' sertifikanın Common Name (CN) veya Subject Alternative Name (SAN)
        // alanıyla eşleşmelidir. Genellikle development'da servis adı kullanılır.
        .domain_name(media_service_host)
        .ca_certificate(server_ca_certificate) // Sunucuyu bu CA ile doğrula
        .identity(client_identity);             // Kendini bu kimlikle sun

    // 5. Sunucuya bağlanmak için güvenli bir kanal (Channel) oluştur
    println!("\nMedia Service'e bağlanılıyor...");
    let channel = Channel::from_shared(server_addr)?
        .tls_config(tls_config)?
        .connect()
        .await?;
        
    println!("✅ Bağlantı başarılı!");

    // 6. gRPC istemcisini oluştur
    let mut client = MediaServiceClient::new(channel);

    // --- AllocatePort RPC Çağrısını Yapalım ---
    println!("\n'AllocatePort' isteği gönderiliyor...");
    let request = tonic::Request::new(AllocatePortRequest {
        call_id: format!("agent-call-{}", rand::random::<u32>()),
    });

    let response = client.allocate_port(request).await?;
    let allocated_port = response.into_inner().rtp_port;
    println!("✅ Başarılı yanıt alındı. Tahsis edilen RTP Portu: {}", allocated_port);

    // Kısa bir bekleme
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // --- ReleasePort RPC Çağrısını Yapalım ---
    println!("\n'ReleasePort' isteği gönderiliyor (Port: {})...", allocated_port);
    let request = tonic::Request::new(ReleasePortRequest {
        rtp_port: allocated_port,
    });

    let response = client.release_port(request).await?;
    println!("✅ Port serbest bırakma yanıtı: success={}", response.into_inner().success);
    
    println!("\n--- Simülasyon Tamamlandı ---");

    Ok(())
}