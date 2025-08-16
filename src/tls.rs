// src/tls.rs

use std::env;
use anyhow::{Result, Context};
use tonic::transport::{Certificate, Identity, ServerTlsConfig};

// Bu fonksiyon artık doğrudan tonic'in ihtiyacı olan ServerTlsConfig'i döndürür.
pub async fn load_server_tls_config() -> Result<ServerTlsConfig> {
    let cert_path = env::var("MEDIA_SERVICE_CERT_PATH").context("MEDIA_SERVICE_CERT_PATH eksik")?;
    let key_path = env::var("MEDIA_SERVICE_KEY_PATH").context("MEDIA_SERVICE_KEY_PATH eksik")?;
    let ca_path = env::var("GRPC_TLS_CA_PATH").context("GRPC_TLS_CA_PATH eksik")?;

    // Sertifika ve anahtarları byte olarak oku
    let server_cert = tokio::fs::read(&cert_path)
        .await
        .context("Sunucu sertifikası okunamadı")?;
    let server_key = tokio::fs::read(&key_path)
        .await
        .context("Sunucu anahtarı okunamadı")?;
    let client_ca = tokio::fs::read(&ca_path)
        .await
        .context("CA sertifikası okunamadı")?;

    // Sunucu kimliğini (sertifika + anahtar) oluştur
    let identity = Identity::from_pem(server_cert, server_key);
    
    // İstemciyi doğrulamak için CA sertifikasını oluştur
    let client_ca_cert = Certificate::from_pem(client_ca);

    // Tonic'in ServerTlsConfig'ini kendi builder'ı ile oluştur
    let tls_config = ServerTlsConfig::new()
        .identity(identity)
        .client_ca_root(client_ca_cert);

    Ok(tls_config)
}