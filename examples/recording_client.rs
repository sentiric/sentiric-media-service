// examples/recording_client.rs

use anyhow::Result;
use std::env;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};
use sentiric_contracts::sentiric::media::v1::{
    media_service_client::MediaServiceClient,
    AllocatePortRequest, ReleasePortRequest, StartRecordingRequest, StopRecordingRequest
};
use tokio::time::{sleep, Duration};

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::from_filename("development.env").ok();
    println!("--- Kalıcı Kayıt İstemci Simülasyonu Başlatılıyor ---");

    // --- TLS ve Kanal Kurulumu (agent_client ile aynı) ---
    let client_cert_path = env::var("AGENT_SERVICE_CERT_PATH")?;
    let client_key_path = env::var("AGENT_SERVICE_KEY_PATH")?;
    let ca_path = env::var("GRPC_TLS_CA_PATH")?;
    let media_service_url = env::var("MEDIA_SERVICE_GRPC_URL")?;
    let server_addr = format!("https://{}", media_service_url);

    let client_identity = Identity::from_pem(tokio::fs::read(&client_cert_path).await?, tokio::fs::read(&client_key_path).await?);
    let server_ca_certificate = Certificate::from_pem(tokio::fs::read(&ca_path).await?);
    let tls_config = ClientTlsConfig::new()
        .domain_name(env::var("MEDIA_SERVICE_HOST")?)
        .ca_certificate(server_ca_certificate)
        .identity(client_identity);
    
    println!("Media Service'e bağlanılıyor: {}", server_addr);
    let channel = Channel::from_shared(server_addr)?.tls_config(tls_config)?.connect().await?;
    let mut client = MediaServiceClient::new(channel);
    println!("✅ Bağlantı başarılı!");

    // --- SENARYO ---

    // 1. Bir port al
    println!("\nAdım 1: Port tahsis ediliyor...");
    let allocate_res = client.allocate_port(AllocatePortRequest {
        call_id: format!("rec-call-{}", rand::random::<u32>()),
    }).await?;
    let rtp_port = allocate_res.into_inner().rtp_port;
    println!("✅ Port tahsis edildi: {}", rtp_port);
    
    // 2. Kaydı başlat
    // Windows için: "C:/Users/YourUser/Desktop/test_kayit.wav" gibi bir yol kullanabilirsiniz.
    // Dizinlerin var olduğundan emin olun veya kodu test edin. Şimdilik geçici dizini kullanalım.
    let mut save_path = env::temp_dir();
    save_path.push("sentiric_recordings");
    save_path.push(format!("rec_port_{}.wav", rtp_port));
    // Dosya URI şemasına çeviriyoruz
    let output_uri = format!("file:///{}", save_path.to_string_lossy().replace('\\', "/"));

    println!("\nAdım 2: Kayıt başlatılıyor. Hedef: {}", output_uri);
    client.start_recording(StartRecordingRequest {
        server_rtp_port: rtp_port,
        output_uri: output_uri.clone(),
        sample_rate: Some(16000),
        format: Some("wav".to_string()),
    }).await?;
    println!("✅ Kayıt başlatma komutu gönderildi.");

    // 3. Bir süre bekle (bu sırada softphone ile ses gönderebilirsiniz)
    println!("\n(Kayıt için 5 saniye bekleniyor... Bu sırada localhost:{} adresine ses gönderebilirsiniz)", rtp_port);
    sleep(Duration::from_secs(5)).await;

    // 4. Kaydı durdur
    println!("\nAdım 3: Kayıt durduruluyor...");
    client.stop_recording(StopRecordingRequest {
        server_rtp_port: rtp_port,
    }).await?;
    println!("✅ Kayıt durdurma komutu gönderildi.");

    // Kaydın diske yazılması için kısa bir süre daha bekle
    sleep(Duration::from_secs(1)).await;

    // 5. Portu serbest bırak
    println!("\nAdım 4: Port serbest bırakılıyor...");
    client.release_port(ReleasePortRequest {
        rtp_port,
    }).await?;
    println!("✅ Port serbest bırakıldı.");
    
    println!("\n--- Simülasyon Tamamlandı ---");
    println!("Kayıt dosyası şu yolda olmalı: {}", save_path.display());
    Ok(())
}