pub mod config;
pub mod state;
pub mod grpc;
pub mod rtp;
pub mod audio;
pub mod tls;

pub use sentiric_contracts::sentiric::media::v1::{
    media_service_server::{MediaService, MediaServiceServer},
    AllocatePortRequest, AllocatePortResponse, PlayAudioRequest, PlayAudioResponse,
    ReleasePortRequest, ReleasePortResponse, RecordAudioRequest, RecordAudioResponse,
};
pub use config::AppConfig;
pub use grpc::service::MyMediaService;

use std::sync::Arc;
use anyhow::{Context, Result};
use tonic::transport::Server;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use state::{AppState, PortManager};


pub async fn run() -> Result<()> {
    // .env dosyasını yüklemeye çalışıyoruz, hata olursa yoksayıyoruz.
    // Artık 'development.env' dosyasını doğru formatta olduğu için bu çalışacak.
    dotenvy::from_filename("development.env").ok();
    dotenvy::dotenv().ok(); // Ek olarak standart .env'yi de arayabilir.
    
    let config = Arc::new(AppConfig::load_from_env().context("Konfigürasyon dosyası yüklenemedi")?);

    let env_filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(&config.rust_log))?;
    
    let subscriber_builder = tracing_subscriber::fmt().with_env_filter(env_filter);

    if config.env == "development" {
        subscriber_builder.with_target(true).with_line_number(true).init();
    } else {
        subscriber_builder.json().with_current_span(true).with_span_list(true).init();
    }
    
    info!("Konfigürasyon başarıyla yüklendi.");

    let tls_config = tls::load_server_tls_config().await
        .context("TLS konfigürasyonu yüklenemedi")?;

    let port_manager = PortManager::new(config.rtp_port_min, config.rtp_port_max);
    let app_state = AppState::new(port_manager.clone());

    let reclamation_manager = app_state.port_manager.clone();
    let quarantine_duration = config.rtp_port_quarantine_duration;
    tokio::spawn(async move {
        reclamation_manager.run_reclamation_task(quarantine_duration).await;
    });

    let media_service = MyMediaService::new(config.clone(), app_state);

    info!(config = ?config, "Media Service hazırlanıyor...");
    
    let server_addr = config.grpc_listen_addr;
    info!(address = %server_addr, "Güvenli gRPC sunucusu dinlemeye başlıyor...");

    Server::builder()
        .tls_config(tls_config)?
        .add_service(MediaServiceServer::new(media_service))
        .serve_with_shutdown(server_addr, shutdown_signal())
        .await?;

    info!("Servis başarıyla durduruldu.");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async { tokio::signal::ctrl_c().await.expect("Failed to install Ctrl+C handler"); };

    #[cfg(unix)]
    let terminate = async { 
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    warn!("Kapatma sinyali alındı. Graceful shutdown başlatılıyor...");
}