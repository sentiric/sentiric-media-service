pub mod config;
pub mod state;
pub mod grpc;
pub mod rtp;
pub mod audio;
pub mod tls;
pub mod metrics;
pub mod rabbitmq;

use config::AppConfig;
use grpc::service::MyMediaService;
use metrics::start_metrics_server;
use sentiric_contracts::sentiric::media::v1::media_service_server::MediaServiceServer;
use state::{AppState, PortManager};
use tls::load_server_tls_config;

use anyhow::{Context, Result};
use aws_config::meta::region::RegionProviderChain;
use aws_config::{BehaviorVersion, Region};
use aws_credential_types::Credentials;
use aws_sdk_s3::config::Builder as S3ConfigBuilder;
use aws_sdk_s3::Client as S3Client;
use std::env;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tonic::transport::Server;
use tracing::{error, info, warn};
use tracing_subscriber::{prelude::*, EnvFilter, fmt::{self, format::FmtSpan}, Registry};

// App struct'ı, uygulamanın tüm durumunu ve yaşam döngüsünü yönetir.
pub struct App {
    config: Arc<AppConfig>,
    shutdown_tx: mpsc::Sender<()>,
    server_handle: JoinHandle<Result<()>>,
}

impl App {
    // bootstrap, uygulamayı yapılandırır ve başlatılmaya hazır hale getirir.
    pub async fn bootstrap() -> Result<Self> {
        // --- 1. Konfigürasyon ve Loglamayı Yükle ---
        let env_file = env::var("ENV_FILE").unwrap_or_else(|_| ".env.docker".to_string());
        if let Err(e) = dotenvy::from_filename(&env_file) {
            warn!(file = %env_file, error = %e, "Ortam değişkenleri dosyası yüklenemedi (bu bir hata olmayabilir).");
        }
        let config = Arc::new(AppConfig::load_from_env().context("Konfigürasyon dosyası yüklenemedi")?);

        let env_filter = EnvFilter::try_from_default_env().or_else(|_| EnvFilter::try_new(&config.rust_log))?;
        let subscriber = Registry::default().with(env_filter);
        if config.env == "development" {
            subscriber.with(fmt::layer().with_target(true).with_line_number(true)).init();
        } else {
            subscriber.with(fmt::layer().json()).init();
        }
        
        let service_version = env::var("SERVICE_VERSION").unwrap_or_else(|_| "0.0.0".to_string());
        let git_commit = env::var("GIT_COMMIT").unwrap_or_else(|_| "unknown".to_string());
        let build_date = env::var("BUILD_DATE").unwrap_or_else(|_| "unknown".to_string());

        info!(
            service_name = "sentiric-media-service", version = %service_version,
            commit = %git_commit, build_date = %build_date, profile = %config.env,
            "🚀 Servis başlatılıyor..."
        );

        // --- 2. Metrik Sunucusunu Başlat ---
        let metrics_addr = SocketAddr::new("0.0.0.0".parse().unwrap(), config.metrics_port);
        start_metrics_server(metrics_addr);

        // --- 3. Uygulamanın Ana Döngüsünü Ayrı Bir Task'te Başlat ---
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel(1);
        let app_config = config.clone();
        let server_handle = tokio::spawn(async move {
            // --- 3a. Arka planda bağımlılıkları kur ---
            let app_state = Self::setup_dependencies(app_config.clone()).await?;

            // --- 3b. Port karantina görevini başlat ---
            let reclamation_manager = app_state.port_manager.clone();
            let quarantine_duration = app_config.rtp_port_quarantine_duration;
            tokio::spawn(async move {
                reclamation_manager.run_reclamation_task(quarantine_duration).await;
            });
            
            // --- 3c. gRPC sunucusunu başlat ---
            let tls_config = load_server_tls_config().await.context("TLS konfigürasyonu yüklenemedi")?;
            let media_service = MyMediaService::new(app_config.clone(), app_state);
            let server_addr = app_config.grpc_listen_addr;
            info!(address = %server_addr, "Güvenli gRPC sunucusu dinlemeye başlıyor...");

            let server = Server::builder()
                .tls_config(tls_config)?
                .add_service(MediaServiceServer::new(media_service))
                .serve_with_shutdown(server_addr, async {
                    shutdown_rx.recv().await;
                    info!("gRPC sunucusu için kapatma sinyali alındı.");
                });
            
            server.await.context("gRPC sunucusu hatayla sonlandı")
        });

        Ok(Self { config, shutdown_tx, server_handle })
    }

    // run, graceful shutdown sinyallerini dinler ve uygulamayı yönetir.
    pub async fn run(self) -> Result<()> {
        let ctrl_c = async { tokio::signal::ctrl_c().await.expect("Failed to install Ctrl+C handler"); };
        #[cfg(unix)]
        let terminate = async { 
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("Failed to install signal handler").recv().await;
        };
        #[cfg(not(unix))]
        let terminate = std::future::pending::<()>();
        
        tokio::select! {
            res = self.server_handle => {
                error!("Sunucu beklenmedik şekilde sonlandı!");
                return res?;
            },
            _ = ctrl_c => {},
            _ = terminate => {},
        }

        warn!("Kapatma sinyali alındı. Graceful shutdown başlatılıyor...");
        let _ = self.shutdown_tx.send(()).await;
        // Sunucunun kapanmasını bekle, ancak timeout ile.
        match tokio::time::timeout(std::time::Duration::from_secs(10), self.server_handle).await {
            Ok(Ok(_)) => info!("Servis başarıyla durduruldu."),
            Ok(Err(e)) => error!(error = %e, "Sunucu durdurulurken hata oluştu."),
            Err(_) => warn!("Sunucuyu durdururken zaman aşımına uğradı."),
        }
        Ok(())
    }

    // setup_dependencies, arka planda dayanıklı bir şekilde bağımlılıkları kurar.
    async fn setup_dependencies(config: Arc<AppConfig>) -> Result<AppState> {
        let s3_client_handle = tokio::spawn(Self::create_s3_client(config.clone()));
        let rabbit_handle = tokio::spawn(Self::create_rabbitmq_channel(config.clone()));

        let s3_client = s3_client_handle.await??;
        let rabbit_channel = rabbit_handle.await??;
        
        let port_manager = PortManager::new(config.rtp_port_min, config.rtp_port_max, config.clone());
        let app_state = AppState::new(port_manager.clone(), s3_client, rabbit_channel);
        Ok(app_state)
    }

    async fn create_s3_client(config: Arc<AppConfig>) -> Result<Option<Arc<S3Client>>> {
        if let Some(s3_config) = &config.s3_config {
            info!("S3 istemcisi oluşturuluyor...");
            let region_provider = RegionProviderChain::first_try(Region::new(s3_config.region.clone()));
            let sdk_config = aws_config::defaults(BehaviorVersion::latest())
                .region(region_provider)
                .endpoint_url(&s3_config.endpoint_url)
                .credentials_provider(Credentials::new(
                    &s3_config.access_key_id, &s3_config.secret_access_key, None, None, "Static",
                )).load().await;
            
            let s3_client_config = S3ConfigBuilder::from(&sdk_config).force_path_style(true).build();
            let client = S3Client::from_conf(s3_client_config);
            
            info!("S3 istemcisi başarıyla oluşturuldu.");
            return Ok(Some(Arc::new(client)));
        }
        warn!("S3 konfigürasyonu bulunamadı, S3 kayıt özelliği devre dışı.");
        Ok(None)
    }

    async fn create_rabbitmq_channel(config: Arc<AppConfig>) -> Result<Option<Arc<lapin::Channel>>> {
        if let Some(url) = &config.rabbitmq_url {
            let channel = rabbitmq::connect_with_retry(url).await?;
            rabbitmq::declare_exchange(&channel).await?;
            info!(exchange_name = rabbitmq::EXCHANGE_NAME, "RabbitMQ exchange'i deklare edildi.");
            return Ok(Some(channel));
        }
        warn!("RABBITMQ_URL bulunamadı, olay yayınlama özelliği devre dışı.");
        Ok(None)
    }
}