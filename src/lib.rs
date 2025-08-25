pub mod config;
pub mod state;
pub mod grpc;
pub mod rtp;
pub mod audio;
pub mod tls;
pub mod metrics;

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
use std::net::SocketAddr;
use crate::metrics::start_metrics_server;
use tracing::{info, warn, error};
use tracing_subscriber::{
    prelude::*,
    EnvFilter,
    fmt::{self, format::FmtSpan},
    Registry
};

use state::{AppState, PortManager};


pub async fn run() -> Result<()> {
    // --- NİHAİ KONFİGÜRASYON YÜKLEYİCİ ---
    // Sadece 'development.env' dosyasını yüklemeyi dener. 
    // Eğer dosya yoksa (Docker ortamı gibi), hiçbir şey yapmaz ve hata vermez.
    // Bu, yerel geliştirme için dosya kullanımına izin verirken, Docker'da
    // sadece ortam değişkenlerine güvenmemizi sağlar.
    let _ = dotenvy::from_filename("development.env");

    let config = Arc::new(AppConfig::load_from_env().context("Konfigürasyon dosyası yüklenemedi")?);

    let metrics_addr = SocketAddr::new(
        "0.0.0.0".parse().unwrap(),
        config.metrics_port,
    );
    start_metrics_server(metrics_addr);

    let env_filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(&config.rust_log))?;
    
    let subscriber = Registry::default().with(env_filter);

    if config.env == "development" {
        let fmt_layer = fmt::layer()
            .with_target(true)
            .with_line_number(true)
            .with_span_events(FmtSpan::FULL);
        subscriber.with(fmt_layer).init();
    } else {
        let fmt_layer = fmt::layer()
            .json()
            .with_current_span(true)
            .with_span_list(true);
        subscriber.with(fmt_layer).init();
    }
    
    info!(service_name = "sentiric-media-service", "Loglama altyapısı başlatıldı.");
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