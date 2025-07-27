use std::collections::HashSet;
use std::env;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use async_mutex::Mutex;
use rand::{thread_rng, Rng};
use tonic::{transport::Server, Request, Response, Status};
use tracing::{info, error, debug, instrument};
use tracing_subscriber::EnvFilter;

use sentiric_contracts::sentiric::media::v1::{
    media_service_server::{MediaService, MediaServiceServer},
    AllocatePortRequest, AllocatePortResponse, ReleasePortRequest, ReleasePortResponse,
};

type PortPool = Arc<Mutex<HashSet<u16>>>;

#[derive(Debug, Clone)]
struct AppConfig {
    grpc_listen_addr: SocketAddr,
    rtp_host: String,
    rtp_port_min: u16,
    rtp_port_max: u16,
}

impl AppConfig {
    fn load_from_env() -> Result<Self, Box<dyn std::error::Error>> {
        dotenv::dotenv().ok();

        let grpc_host = "0.0.0.0".to_string();
        let grpc_port_str = env::var("INTERNAL_GRPC_PORT_MEDIA")?;
        let grpc_port = grpc_port_str.parse::<u16>()?;
        
        let rtp_host = "0.0.0.0".to_string();
        let rtp_port_min_str = env::var("EXTERNAL_RTP_PORT_MIN")?;
        let rtp_port_max_str = env::var("EXTERNAL_RTP_PORT_MAX")?;
        
        Ok(AppConfig {
            grpc_listen_addr: format!("{}:{}", grpc_host, grpc_port).parse()?,
            rtp_host,
            rtp_port_min: rtp_port_min_str.parse()?,
            rtp_port_max: rtp_port_max_str.parse()?,
        })
    }
}

pub struct MyMediaService {
    allocated_ports: PortPool,
    config: Arc<AppConfig>,
}

impl MyMediaService {
    fn new(config: Arc<AppConfig>) -> Self {
        Self {
            allocated_ports: Arc::new(Mutex::new(HashSet::new())),
            config,
        }
    }
}

#[tonic::async_trait]
impl MediaService for MyMediaService {
    // 'instrument' makrosunu kaldırdık ve loglamayı manuel yaparak daha net kontrol sağlıyoruz.
    async fn allocate_port(&self, request: Request<AllocatePortRequest>) -> Result<Response<AllocatePortResponse>, Status> {
        let call_id = &request.get_ref().call_id;
        // Değişkeni burada loglayarak "kullanılmış" hale getiriyoruz ve uyarıyı gideriyoruz.
        info!(call_id = %call_id, "AllocatePort isteği alındı.");
        
        let mut ports_guard = self.allocated_ports.lock().await;
        for _ in 0..100 {
            let port = thread_rng().gen_range(self.config.rtp_port_min..=self.config.rtp_port_max);
            let rtp_port = if port % 2 == 0 { port } else { port.saturating_add(1) };
            if rtp_port > self.config.rtp_port_max { continue; }

            if !ports_guard.contains(&rtp_port) {
                let bind_addr = format!("{}:{}", self.config.rtp_host, rtp_port);
                if let Ok(socket) = UdpSocket::bind(&bind_addr).await {
                    info!(port = rtp_port, "Boş port bulundu ve bağlandı.");
                    ports_guard.insert(rtp_port);
                    tokio::spawn(handle_rtp_stream(socket));
                    return Ok(Response::new(AllocatePortResponse { rtp_port: rtp_port as u32 }));
                }
            }
        }
        error!("Uygun RTP portu bulunamadı.");
        Err(Status::resource_exhausted("Available RTP port pool is exhausted."))
    }
    
    #[instrument(skip(self), fields(port = %request.get_ref().rtp_port))]
    async fn release_port(&self, request: Request<ReleasePortRequest>) -> Result<Response<ReleasePortResponse>, Status> {
        info!("ReleasePort isteği alındı.");
        let port_to_release = request.into_inner().rtp_port as u16;
        let mut ports_guard = self.allocated_ports.lock().await;
        if ports_guard.remove(&port_to_release) {
            info!(port = port_to_release, "Port başarıyla serbest bırakıldı.");
            Ok(Response::new(ReleasePortResponse { success: true }))
        } else {
            info!(port = port_to_release, "Serbest bırakılacak port listede bulunamadı.");
            Ok(Response::new(ReleasePortResponse { success: false }))
        }
    }
}

async fn handle_rtp_stream(socket: UdpSocket) {
    let local_addr = socket.local_addr().unwrap();
    info!(%local_addr, "Yeni RTP stream için dinleyici başlatıldı.");
    let mut buf = [0; 2048];
    loop {
        match socket.recv_from(&mut buf).await {
            Ok((len, remote_addr)) => {
                debug!(port = local_addr.port(), %remote_addr, bytes = len, "RTP paketi alındı.");
            }
            Err(e) => {
                error!(error = %e, "RTP soket dinleme hatası.");
                break;
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::fmt()
        .json()
        .with_env_filter(env_filter)
        .init();

    let config = match AppConfig::load_from_env() {
        Ok(cfg) => Arc::new(cfg),
        Err(e) => {
            error!(error = %e, "Konfigürasyon yüklenemedi. .env veya ortam değişkenlerini kontrol edin.");
            return Err(e);
        }
    };
    info!(config = ?config, "Media Service başlatılıyor.");
    
    let media_service = MyMediaService::new(config.clone());
    
    Server::builder()
        .add_service(MediaServiceServer::new(media_service))
        .serve(config.grpc_listen_addr)
        .await?;

    Ok(())
}