// sentiric-media-service/src/tls.rs
use crate::config::AppConfig;
use anyhow::{Context, Result};
use tonic::transport::{Certificate, Identity, ServerTlsConfig};

// DÜZELTME: Fonksiyon artık AppConfig'i parametre olarak alıyor.
pub async fn load_server_tls_config(config: &AppConfig) -> Result<ServerTlsConfig> {
    let identity = {
        // DÜZELTME: Doğrudan config nesnesinden okuyor.
        let cert = tokio::fs::read(&config.cert_path).await.context("Sunucu sertifikası okunamadı")?;
        let key = tokio::fs::read(&config.key_path).await.context("Sunucu anahtarı okunamadı")?;
        Identity::from_pem(cert, key)
    };
    
    let client_ca_cert = {
        // DÜZELTME: Doğrudan config nesnesinden okuyor.
        let ca = tokio::fs::read(&config.ca_path).await.context("CA sertifikası okunamadı")?;
        Certificate::from_pem(ca)
    };

    Ok(ServerTlsConfig::new()
        .identity(identity)
        .client_ca_root(client_ca_cert))
}