// File: src/state.rs
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, info};
use aws_sdk_s3::Client as S3Client;
use lapin::Channel as LapinChannel;
use crate::audio::AudioCache;
use crate::rtp::command::RtpCommand;
use crate::config::AppConfig; // AppConfig'i import ediyoruz

type SessionChannels = Arc<Mutex<HashMap<u16, mpsc::Sender<RtpCommand>>>>;
type PortsPool = Arc<Mutex<Vec<u16>>>;
type QuarantinedPorts = Arc<Mutex<Vec<(u16, Instant)>>>;

#[derive(Clone)]
pub struct AppState {
    pub port_manager: PortManager,
    pub audio_cache: AudioCache,
    pub s3_client: Option<Arc<S3Client>>,
    pub rabbitmq_publisher: Option<Arc<LapinChannel>>,
}

impl AppState {
    pub fn new(
        port_manager: PortManager, 
        s3_client: Option<Arc<S3Client>>,
        rabbitmq_publisher: Option<Arc<LapinChannel>>
    ) -> Self {
        Self {
            port_manager,
            audio_cache: Arc::new(Mutex::new(HashMap::new())),
            s3_client,
            rabbitmq_publisher,
        }
    }
}

#[derive(Clone)]
pub struct PortManager {
    session_channels: SessionChannels,
    available_ports: PortsPool,
    quarantined_ports: QuarantinedPorts,
    // --- DEĞİŞİKLİK BURADA: Config'i PortManager'a ekliyoruz ---
    pub config: Arc<AppConfig>, 
}

impl PortManager {
    // --- DEĞİŞİKLİK BURADA: new fonksiyonu artık config alıyor ---
    pub fn new(rtp_port_min: u16, rtp_port_max: u16, config: Arc<AppConfig>) -> Self {
        let initial_ports: Vec<u16> = (rtp_port_min..=rtp_port_max).filter(|&p| p % 2 == 0).collect();
        info!(port_count = initial_ports.len(), "Kullanılabilir port havuzu oluşturuldu.");
        Self {
            session_channels: Arc::new(Mutex::new(HashMap::new())),
            available_ports: Arc::new(Mutex::new(initial_ports)),
            quarantined_ports: Arc::new(Mutex::new(Vec::new())),
            config, // ve burada atıyoruz
        }
    }

    pub async fn get_available_port(&self) -> Option<u16> {
        self.available_ports.lock().await.pop()
    }

    pub async fn add_session(&self, port: u16, tx: mpsc::Sender<RtpCommand>) {
        self.session_channels.lock().await.insert(port, tx);
    }
    
    pub async fn get_session_sender(&self, port: u16) -> Option<mpsc::Sender<RtpCommand>> {
        self.session_channels.lock().await.get(&port).cloned()
    }
    
    pub async fn remove_session(&self, port: u16) {
        self.session_channels.lock().await.remove(&port);
    }

    pub async fn quarantine_port(&self, port: u16) {
        self.quarantined_ports.lock().await.push((port, Instant::now()));
    }
    
    pub async fn run_reclamation_task(&self, cooldown: Duration) {
        info!("Port karantina temizleme görevi başlatıldı. Soğuma süresi: {:?}", cooldown);
        let mut interval = tokio::time::interval(cooldown);
        loop {
            interval.tick().await;
            let mut quarantined_guard = self.quarantined_ports.lock().await;
            if quarantined_guard.is_empty() { continue; }
            
            let now = Instant::now();
            let mut available_ports_guard = self.available_ports.lock().await;
            
            quarantined_guard.retain(|(port, release_time)| {
                if now.duration_since(*release_time) >= cooldown {
                    debug!(port, "Port karantinadan çıkarıldı ve havuza eklendi.");
                    available_ports_guard.push(*port);
                    false
                } else {
                    true
                }
            });
        }
    }
}