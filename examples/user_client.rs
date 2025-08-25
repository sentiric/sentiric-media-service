// examples/user_client.rs

use anyhow::Result;
use std::env;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};

// RPC çağrısı yapmayacağımız için sadece temel tonic tipleri yeterli.
// İstemci veya request/response tiplerini import etmiyoruz.

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
    
    println!("--- User Service İstemci Simülasyonu Başlatılıyor ---");
    println!("--- SENARYO: Sadece Media Service'e bağlanılabildiği test edilecek. ---");

    // 1. Sertifika yollarını user-service için güncelle
    let client_cert_path = env::var("USER_SERVICE_CERT_PATH")?;
    let client_key_path = env::var("USER_SERVICE_KEY_PATH")?;
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

    // 3. Sadece bağlanmayı dene
    println!("\nMedia Service'e bağlanılıyor...");
    let _channel = Channel::from_shared(server_addr)?.tls_config(tls_config)?.connect().await?;
    println!("✅ Bağlantı başarılı! User Service'in Media Service'e erişim yetkisi var.");

    // Hiçbir RPC çağrısı yapmadan çık.
    println!("\n--- Simülasyon Tamamlandı ---");
    Ok(())
}