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
use crate::config::AppConfig;

type SessionChannels = Arc<Mutex<HashMap<u16, mpsc::Sender<RtpCommand>>>>;
// YENİ: Call ID -> Port eşleşmesi
type CallIdMap = Arc<Mutex<HashMap<String, u16>>>; 
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
    call_id_map: CallIdMap, // YENİ
    available_ports: PortsPool,
    quarantined_ports: QuarantinedPorts,
    pub config: Arc<AppConfig>, 
}

impl PortManager {
    pub fn new(rtp_port_min: u16, rtp_port_max: u16, config: Arc<AppConfig>) -> Self {
        let initial_ports: Vec<u16> = (rtp_port_min..=rtp_port_max).filter(|&p| p % 2 == 0).collect();
        info!(port_count = initial_ports.len(), "Kullanılabilir port havuzu oluşturuldu.");
        Self {
            session_channels: Arc::new(Mutex::new(HashMap::new())),
            call_id_map: Arc::new(Mutex::new(HashMap::new())), // YENİ
            available_ports: Arc::new(Mutex::new(initial_ports)),
            quarantined_ports: Arc::new(Mutex::new(Vec::new())),
            config,
        }
    }

    pub async fn get_available_port(&self) -> Option<u16> {
        self.available_ports.lock().await.pop()
    }

    // GÜNCELLENDİ: Call ID opsiyonel olarak alınır ve saklanır
    pub async fn add_session(&self, port: u16, tx: mpsc::Sender<RtpCommand>, call_id: Option<String>) {
        self.session_channels.lock().await.insert(port, tx);
        if let Some(cid) = call_id {
            self.call_id_map.lock().await.insert(cid, port);
        }
    }
    
    pub async fn get_session_sender(&self, port: u16) -> Option<mpsc::Sender<RtpCommand>> {
        self.session_channels.lock().await.get(&port).cloned()
    }

    // YENİ: Call ID'den portu bul
    pub async fn get_port_by_call_id(&self, call_id: &str) -> Option<u16> {
        self.call_id_map.lock().await.get(call_id).cloned()
    }
    
    pub async fn remove_session(&self, port: u16) {
        self.session_channels.lock().await.remove(&port);
        // Call ID map'ten silmek için ters arama yapmak verimsiz olabilir, 
        // ancak port sayısı sınırlı olduğu için kabul edilebilir.
        // Veya session kapatılırken call_id'yi bilmemiz gerekirdi.
        // Hızlı çözüm: Map'i iterate et.
        let mut map = self.call_id_map.lock().await;
        map.retain(|_, &mut v| v != port);
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