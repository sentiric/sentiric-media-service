// examples/shared/grpc_client.rs
use anyhow::Result;
use sentiric_contracts::sentiric::media::v1::media_service_client::MediaServiceClient;
use std::env;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};

pub async fn connect_to_media_service() -> Result<MediaServiceClient<Channel>> {
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
    
    let channel = Channel::from_shared(server_addr)?
        .tls_config(tls_config)?
        .connect()
        .await?;
        
    Ok(MediaServiceClient::new(channel))
}