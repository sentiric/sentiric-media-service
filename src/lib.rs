// File: src/lib.rs
pub mod config;
pub mod state;
pub mod grpc;
pub mod rtp;
pub mod audio;
pub mod tls;
pub mod metrics;
pub mod rabbitmq;

pub use sentiric_contracts::sentiric::media::v1::{
    media_service_server::{MediaService, MediaServiceServer},
    AllocatePortRequest, AllocatePortResponse, PlayAudioRequest, PlayAudioResponse,
    ReleasePortRequest, ReleasePortResponse, RecordAudioRequest, RecordAudioResponse,
    StartRecordingRequest, StartRecordingResponse, StopRecordingRequest, StopRecordingResponse,
};
pub use config::AppConfig;
pub use grpc::service::MyMediaService;

use std::sync::Arc;
use anyhow::{Context, Result};
use tonic::transport::Server;
use std::net::SocketAddr;
use crate::metrics::start_metrics_server;
use tracing::{info, warn};
use tracing_subscriber::{prelude::*, EnvFilter, fmt::{self, format::FmtSpan}, Registry};
use state::{AppState, PortManager};
use std::env;
use aws_config::meta::region::RegionProviderChain;
use aws_config::{BehaviorVersion, Region};
use aws_credential_types::Credentials;
use aws_sdk_s3::config::Builder as S3ConfigBuilder;
use aws_sdk_s3::Client as S3Client;

async fn create_s3_client(config: &AppConfig) -> Result<Option<Arc<S3Client>>> {
    if let Some(s3_config) = &config.s3_config {
        info!("S3 konfigürasyonu bulundu, S3 istemcisi oluşturuluyor...");
        let region_provider = RegionProviderChain::first_try(Region::new(s3_config.region.clone()));
        let sdk_config = aws_config::defaults(BehaviorVersion::latest())
            .region(region_provider)
            .endpoint_url(&s3_config.endpoint_url)
            .credentials_provider(Credentials::new(
                &s3_config.access_key_id,
                &s3_config.secret_access_key,
                None,
                None,
                "Static",
            ))
            .load()
            .await;
        
        let s3_client_config = S3ConfigBuilder::from(&sdk_config)
            .force_path_style(true)
            .build();
        let client = S3Client::from_conf(s3_client_config);
        
        info!("S3 istemcisi başarıyla oluşturuldu.");
        return Ok(Some(Arc::new(client)));
    }
    warn!("S3 konfigürasyonu bulunamadı, S3 kayıt özelliği devre dışı.");
    Ok(None)
}

pub async fn run() -> Result<()> {
    let env_file = env::var("ENV_FILE").unwrap_or_else(|_| ".env.docker".to_string());
    if let Err(e) = dotenvy::from_filename(&env_file) {
        warn!(file = %env_file, error = %e, "Ortam değişkenleri dosyası yüklenemedi (bu bir hata olmayabilir).");
    } else {
        info!(file = %env_file, "Ortam değişkenleri dosyası başarıyla yüklendi.");
    }
    let config = Arc::new(AppConfig::load_from_env().context("Konfigürasyon dosyası yüklenemedi")?);

    let metrics_addr = SocketAddr::new("0.0.0.0".parse().unwrap(), config.metrics_port);
    start_metrics_server(metrics_addr);

    let env_filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(&config.rust_log))?;
    let subscriber = Registry::default().with(env_filter);

    if config.env == "development" {
        let fmt_layer = fmt::layer().with_target(true).with_line_number(true).with_span_events(FmtSpan::NONE);
        subscriber.with(fmt_layer).init();
    } else {
        let fmt_layer = fmt::layer().json().with_current_span(true).with_span_events(FmtSpan::NONE); 
        subscriber.with(fmt_layer).init();
    }
    
    // YENİ: Build-time değişkenlerini environment'tan oku
    let service_version = env::var("SERVICE_VERSION").unwrap_or_else(|_| "0.0.0".to_string());
    let git_commit = env::var("GIT_COMMIT").unwrap_or_else(|_| "unknown".to_string());
    let build_date = env::var("BUILD_DATE").unwrap_or_else(|_| "unknown".to_string());

    // YENİ: Başlangıçta versiyon bilgisini logla
    info!(
        service_name = "sentiric-media-service",
        version = %service_version,
        commit = %git_commit,
        build_date = %build_date,
        profile = %config.env,
        "🚀 Servis başlatılıyor..."
    );
    
    let tls_config = tls::load_server_tls_config().await.context("TLS konfigürasyonu yüklenemedi")?;
    
    let port_manager = PortManager::new(config.rtp_port_min, config.rtp_port_max, config.clone());
    
    let s3_client = create_s3_client(&config).await?;
    
    let rabbit_channel = if let Some(url) = &config.rabbitmq_url {
        let channel = rabbitmq::connect_with_retry(url).await?;
        rabbitmq::declare_exchange(&channel).await?;
        info!(exchange_name = rabbitmq::EXCHANGE_NAME, "RabbitMQ exchange'i deklare edildi.");
        Some(channel)
    } else {
        warn!("RABBITMQ_URL bulunamadı, olay yayınlama özelliği devre dışı.");
        None
    };
    
    let app_state = AppState::new(port_manager.clone(), s3_client, rabbit_channel);

    let reclamation_manager = app_state.port_manager.clone();
    let quarantine_duration = config.rtp_port_quarantine_duration;
    tokio::spawn(async move {
        reclamation_manager.run_reclamation_task(quarantine_duration).await;
    });

    let media_service = MyMediaService::new(config.clone(), app_state);
    
    let server_addr = config.grpc_listen_addr;
    info!(address = %server_addr, "Güvenli gRPC sunucusu dinlemeye başlıyor...");

    let grpc_server = Server::builder()
        .tls_config(tls_config)?
        .add_service(MediaServiceServer::new(media_service))
        .serve_with_shutdown(server_addr, shutdown_signal());
    grpc_server.await?;

    info!("Servis başarıyla durduruldu.");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async { tokio::signal::ctrl_c().await.expect("Failed to install Ctrl+C handler"); };
    #[cfg(unix)]
    let terminate = async { 
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to install signal handler").recv().await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    warn!("Kapatma sinyali alındı. Graceful shutdown başlatılıyor...");
}



#[cfg(test)]
mod tests {
    use super::*;
    use sentiric_contracts::sentiric::media::v1::media_service_client::MediaServiceClient;
    use tokio::time::{sleep, Duration};

    #[tokio::test]
    #[ignore]
    async fn test_start_recording_rpc_exists() {
        let _ = dotenvy::from_filename("development.env");
        tokio::spawn(async { run().await.unwrap(); });
        sleep(Duration::from_secs(2)).await;

        let client_cert_path = std::env::var("AGENT_SERVICE_CERT_PATH").unwrap();
        let client_key_path = std::env::var("AGENT_SERVICE_KEY_PATH").unwrap();
        let ca_path = std::env::var("GRPC_TLS_CA_PATH").unwrap();
        let media_service_url = std::env::var("MEDIA_SERVICE_GRPC_URL").unwrap();
        let server_addr = format!("https://{}", media_service_url);

        let client_identity = tonic::transport::Identity::from_pem(
            tokio::fs::read(&client_cert_path).await.unwrap(),
            tokio::fs::read(&client_key_path).await.unwrap(),
        );
        let server_ca_certificate = tonic::transport::Certificate::from_pem(
            tokio::fs::read(&ca_path).await.unwrap(),
        );
        let tls_config = tonic::transport::ClientTlsConfig::new()
            .domain_name(std::env::var("MEDIA_SERVICE_HOST").unwrap())
            .ca_certificate(server_ca_certificate)
            .identity(client_identity);
        
        let channel = tonic::transport::Channel::from_shared(server_addr)
            .unwrap()
            .tls_config(tls_config)
            .unwrap()
            .connect()
            .await
            .expect("İstemci bağlanamadı");

        let mut client = MediaServiceClient::new(channel);
        let allocate_res = client.allocate_port(AllocatePortRequest {
            call_id: "internal-test-call".to_string(),
        }).await.expect("AllocatePort başarısız");
        let rtp_port = allocate_res.into_inner().rtp_port;

        let result = client.start_recording(StartRecordingRequest {
            server_rtp_port: rtp_port,
            output_uri: "file:///test.wav".to_string(),
            sample_rate: Some(16000),
            format: Some("wav".to_string()),
            call_id: "internal-test-call".to_string(),
            trace_id: "internal-test-trace".to_string(),
        }).await;

        assert!(result.is_ok(), "StartRecording RPC çağrısı başarısız oldu: {:?}", result.err());
        let response_status = result.unwrap().into_inner();
        assert!(response_status.success, "StartRecording yanıtı 'success: false' döndürdü.");
        
        println!("✅ Dahili RPC testi BAŞARILI: Sunucu 'start_recording' metodunu implemente etmiş.");
    }
}