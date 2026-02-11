// sentiric-media-service/src/app.rs
use crate::config::AppConfig;
use crate::grpc::service::MyMediaService;
use crate::metrics::start_metrics_server;
use crate::rabbitmq;
use crate::state::{AppState, PortManager};
use crate::tls::load_server_tls_config;
use sentiric_contracts::sentiric::media::v1::media_service_server::MediaServiceServer;

use anyhow::{Context, Result};
use aws_config::{BehaviorVersion, Region};
use aws_credential_types::Credentials;
use aws_sdk_s3::config::Builder as S3ConfigBuilder;
use aws_sdk_s3::Client as S3Client;
use std::env;
use std::sync::Arc;
use tokio::sync::mpsc;
use tonic::transport::Server;
use tracing::{info, warn};

pub struct App {
    config: Arc<AppConfig>,
}

impl App {
    pub async fn bootstrap() -> Result<Self> {
        let env_file = env::var("ENV_FILE").unwrap_or_else(|_| ".env".to_string());
        if let Err(_) = dotenvy::from_filename(&env_file) {
            warn!(file = %env_file, "Environment file not found, continuing with system env.");
        }

        let config = Arc::new(AppConfig::load_from_env().context("Konfigürasyon dosyası yüklenemedi")?);

        let rust_log_env = env::var("RUST_LOG")
            .unwrap_or_else(|_| "info,h2=warn,hyper=warn,tower=warn,rustls=warn,lapin=warn".to_string());
        
        let env_filter = EnvFilter::try_from_default_env().or_else(|_| EnvFilter::try_new(&rust_log_env))?;
        let subscriber = Registry::default().with(env_filter).with(fmt::layer().json());
        
        tracing::subscriber::set_global_default(subscriber).ok();

        // Metrik sunucusunu başlat
        let metrics_addr = format!("0.0.0.0:{}", config.metrics_port).parse()?;
        start_metrics_server(metrics_addr);

        Ok(Self { config })
    }

    pub async fn run(self) -> Result<()> {
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel(1);
        let app_config = self.config.clone();

        let _server_handle = tokio::spawn(async move {
            let app_state = Self::setup_dependencies(app_config.clone()).await?;
            let reclamation_manager = app_state.port_manager.clone();
            let quarantine_duration = app_config.rtp_port_quarantine_duration;
            tokio::spawn(async move {
                reclamation_manager.run_reclamation_task(quarantine_duration).await;
            });
            
            let tls_config = load_server_tls_config(&app_config).await.context("TLS konfigürasyonu yüklenemedi")?;
            
            let media_service = MyMediaService::new(app_config.clone(), app_state);
            let server_addr = app_config.grpc_listen_addr;
            info!(address = %server_addr, "🚀 Secured gRPC server starting...");

            let server = Server::builder()
                .tls_config(tls_config)?
                .add_service(MediaServiceServer::new(media_service))
                .serve_with_shutdown(server_addr, async {
                    shutdown_rx.recv().await;
                });
            
            server.await.context("gRPC sunucusu hatayla sonlandı")
        });

        tokio::signal::ctrl_c().await.ok();
        warn!("Graceful shutdown initiated...");
        let _ = shutdown_tx.send(()).await;
        
        Ok(())
    }

    async fn setup_dependencies(config: Arc<AppConfig>) -> Result<AppState> {
        let s3_client = Self::create_s3_client(config.clone()).await?;
        let rabbit_channel = Self::create_rabbitmq_channel(config.clone()).await?;
        let port_manager = PortManager::new(config.rtp_port_min, config.rtp_port_max, config.clone());
        
        let app_state = AppState::new(port_manager, s3_client, rabbit_channel);
        
        // 🚀 Background Persistence Worker başlatılıyor
        let worker_state = app_state.clone();
        tokio::spawn(async move {
            let worker = crate::persistence::worker::UploadWorker::new(worker_state);
            worker.run().await;
        });

        Ok(app_state)
    }

    async fn create_s3_client(config: Arc<AppConfig>) -> Result<Option<Arc<S3Client>>> {
        if let Some(s3_config) = &config.s3_config {
            let region = Region::new(s3_config.region.clone());
            let sdk_config = aws_config::defaults(BehaviorVersion::latest())
                .region(region)
                .endpoint_url(&s3_config.endpoint_url)
                .credentials_provider(Credentials::new(
                    &s3_config.access_key_id, &s3_config.secret_access_key, None, None, "Static",
                )).load().await;
            
            let s3_client_config = S3ConfigBuilder::from(&sdk_config).force_path_style(true).build();
            return Ok(Some(Arc::new(S3Client::from_conf(s3_client_config))));
        }
        Ok(None)
    }

    async fn create_rabbitmq_channel(config: Arc<AppConfig>) -> Result<Option<Arc<lapin::Channel>>> {
        if let Some(url) = &config.rabbitmq_url {
            let channel = rabbitmq::connect_with_retry(url).await?;
            rabbitmq::declare_exchange(&channel).await?;
            return Ok(Some(channel));
        }
        Ok(None)
    }
}

// KRİTİK: Importlar sadece üstte tutulur
use tracing_subscriber::{fmt, prelude::*, EnvFilter, Registry};