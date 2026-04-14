// sentiric-media-service/src/state.rs (Üst kısımdaki değişen yer)
use crate::audio::AudioCache;
use crate::config::AppConfig;
use crate::rtp::session::RtpSession;
use aws_sdk_s3::Client as S3Client;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tracing::{debug, info};

type ActiveSessions = Arc<Mutex<HashMap<u16, Arc<RtpSession>>>>;
type PortsPool = Arc<Mutex<VecDeque<u16>>>;
type QuarantinedPorts = Arc<Mutex<Vec<(u16, Instant)>>>;

#[derive(Clone)]
pub struct AppState {
    pub port_manager: PortManager,
    pub audio_cache: AudioCache,
    pub s3_client: Option<Arc<S3Client>>,
    // [HATA BURADAYDI, LapinChannel yerine RabbitMqClient kullanıyoruz]
    pub rabbitmq_publisher: Option<Arc<crate::rabbitmq::RabbitMqClient>>,
}

impl AppState {
    pub fn new(
        port_manager: PortManager,
        s3_client: Option<Arc<S3Client>>,
        rabbitmq_publisher: Option<Arc<crate::rabbitmq::RabbitMqClient>>,
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
    active_sessions: ActiveSessions,
    available_ports: PortsPool,
    quarantined_ports: QuarantinedPorts,
    pub config: Arc<AppConfig>,
}

// [ARCH-COMPLIANCE] Sadece değiştirilen blok
impl PortManager {
    pub fn new(rtp_port_min: u16, rtp_port_max: u16, config: Arc<AppConfig>) -> Self {
        let initial_vec: Vec<u16> = (rtp_port_min..=rtp_port_max)
            .filter(|&p| p % 2 == 0)
            .collect();
        let initial_ports = VecDeque::from(initial_vec);

        info!(
            event = "PORT_POOL_CREATED",
            port_count = initial_ports.len(),
            "Kullanılabilir port havuzu oluşturuldu."
        );

        Self {
            active_sessions: Arc::new(Mutex::new(HashMap::new())),
            available_ports: Arc::new(Mutex::new(initial_ports)),
            quarantined_ports: Arc::new(Mutex::new(Vec::new())),
            config,
        }
    }

    pub async fn get_available_port(&self) -> Option<u16> {
        self.available_ports.lock().await.pop_front()
    }

    // Session nesnesini kaydet
    pub async fn add_session(&self, port: u16, session: Arc<RtpSession>) {
        self.active_sessions.lock().await.insert(port, session);
    }

    // Port'a göre session'ı döndürür
    pub async fn get_session(&self, port: u16) -> Option<Arc<RtpSession>> {
        self.active_sessions.lock().await.get(&port).cloned()
    }

    // CallID'ye göre session'ı bul
    pub async fn get_session_by_call_id(&self, call_id: &str) -> Option<Arc<RtpSession>> {
        let sessions = self.active_sessions.lock().await;
        sessions.values().find(|s| s.call_id == call_id).cloned()
    }

    pub async fn remove_session(&self, port: u16) {
        self.active_sessions.lock().await.remove(&port);
    }

    pub async fn quarantine_port(&self, port: u16) {
        self.quarantined_ports
            .lock()
            .await
            .push((port, Instant::now()));
    }

    pub async fn run_reclamation_task(&self, cooldown: Duration) {
        info!(
            event = "QUARANTINE_TASK_START",
            cooldown_sec = cooldown.as_secs(),
            "Port karantina temizleme görevi başlatıldı."
        );

        let mut interval = tokio::time::interval(cooldown);
        loop {
            interval.tick().await;
            let mut quarantined_guard = self.quarantined_ports.lock().await;
            if quarantined_guard.is_empty() {
                continue;
            }

            let now = Instant::now();
            let mut available_ports_guard = self.available_ports.lock().await;

            quarantined_guard.retain(|(port, release_time)| {
                if now.duration_since(*release_time) >= cooldown {
                    // [ARCH-COMPLIANCE] SUTS v4.2: Rutin port temizliği detayı DEBUG'a çekildi.
                    debug!(
                        event = "PORT_RELEASED_FROM_QUARANTINE",
                        port = port,
                        "Port karantinadan çıkarıldı ve havuza eklendi."
                    );
                    available_ports_guard.push_back(*port);
                    false
                } else {
                    true
                }
            });
        }
    }
}
