// DOSYA: sentiric-media-service/src/main.rs (NİHAİ - TÜM DERLEME HATALARI GİDERİLMİŞ)

use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rand::Rng; // Sadece `Rng` gerekli
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, Mutex};
use tokio::time::{sleep, Instant};
use tonic::{transport::Server, Request, Response, Status};
use tracing::{debug, error, info, instrument, warn};
use tracing_subscriber::EnvFilter;

use rtp::header::Header;
use rtp::packet::Packet;
use webrtc_util::marshal::Marshal;

use sentiric_contracts::sentiric::media::v1::{
    media_service_server::{MediaService, MediaServiceServer},
    AllocatePortRequest, AllocatePortResponse, PlayAudioRequest, PlayAudioResponse,
    ReleasePortRequest, ReleasePortResponse,
};

// --- TYPEDEFS ---
type AudioCache = Arc<Mutex<HashMap<String, Arc<Vec<i16>>>>>;
type ActiveSessionChannels = Arc<Mutex<HashMap<u16, mpsc::Sender<RtpCommand>>>>;
type AvailablePortsPool = Arc<Mutex<Vec<u16>>>;
type QuarantinedPorts = Arc<Mutex<Vec<(u16, Instant)>>>;

// --- ENUMS ---
#[derive(Debug)]
enum RtpCommand {
    PlayFile {
        audio_id: String,
        candidate_target_addr: SocketAddr,
    },
    Shutdown,
}

// --- CONFIG ---
#[derive(Debug, Clone)]
struct AppConfig {
    grpc_listen_addr: SocketAddr,
    rtp_host: String,
    rtp_port_min: u16,
    rtp_port_max: u16,
    assets_base_path: String,
}

impl AppConfig {
    fn load_from_env() -> Result<Self, Box<dyn Error>> {
        let get_env_var = |name: &str| env::var(name).map_err(|e| format!("CRITICAL: Ortam değişkeni '{}' bulunamadı: {}", name, e));
        let grpc_port: u16 = get_env_var("INTERNAL_GRPC_PORT_MEDIA")?.parse()?;
        let rtp_port_min: u16 = get_env_var("EXTERNAL_RTP_PORT_MIN")?.parse()?;
        let rtp_port_max: u16 = get_env_var("EXTERNAL_RTP_PORT_MAX")?.parse()?;
        if rtp_port_min >= rtp_port_max { return Err("CRITICAL: RTP port aralığı geçersiz.".into()); }
        Ok(AppConfig {
            grpc_listen_addr: format!("[::]:{}", grpc_port).parse()?,
            rtp_host: env::var("RTP_HOST").unwrap_or_else(|_| "0.0.0.0".to_string()),
            assets_base_path: env::var("ASSETS_BASE_PATH").unwrap_or_else(|_| "/app/assets".to_string()),
            rtp_port_min,
            rtp_port_max,
        })
    }
}

// --- SERVICE ---
pub struct MyMediaService {
    session_channels: ActiveSessionChannels,
    available_ports: AvailablePortsPool,
    quarantined_ports: QuarantinedPorts,
    config: Arc<AppConfig>,
    audio_cache: AudioCache,
}

impl MyMediaService {
    fn new(config: Arc<AppConfig>) -> Self {
        let initial_ports: Vec<u16> = (config.rtp_port_min..=config.rtp_port_max).filter(|&p| p % 2 == 0).collect();
        info!(port_count = initial_ports.len(), "Kullanılabilir port havuzu oluşturuldu.");
        Self {
            session_channels: Arc::new(Mutex::new(HashMap::new())),
            available_ports: Arc::new(Mutex::new(initial_ports)),
            quarantined_ports: Arc::new(Mutex::new(Vec::new())),
            config,
            audio_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

// --- MAIN ---
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    dotenvy::dotenv().ok();
    
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,sentiric_media_service=debug"));
    tracing_subscriber::fmt().with_env_filter(env_filter).init();
    std::panic::set_hook(Box::new(|panic_info| {
        error!(panic_info = %panic_info, "PROGRAM PANIK YAŞADI! UYGULAMA DURDURULACAK.");
    }));

    info!("Konfigürasyon yükleniyor...");
    let config = Arc::new(AppConfig::load_from_env().unwrap_or_else(|e| {
        error!(error = %e, "KRITIK: Konfigürasyon yüklenemedi. Uygulama kapanacak.");
        std::process::exit(1);
    }));
    
    info!(config = ?config, "Media Service hazırlanıyor...");
    let media_service = MyMediaService::new(config.clone());
    
    tokio::spawn(port_reclamation_task(
        media_service.available_ports.clone(),
        media_service.quarantined_ports.clone(),
        Duration::from_secs(3),
    ));

    let server_addr = media_service.config.grpc_listen_addr;
    info!(address = %server_addr, "gRPC sunucusu dinlemeye başlıyor...");
    Server::builder()
        .add_service(MediaServiceServer::new(media_service))
        .serve_with_shutdown(server_addr, shutdown_signal())
        .await?;

    info!("Servis başarıyla durduruldu.");
    Ok(())
}

// --- BACKGROUND TASKS ---
async fn port_reclamation_task(available_ports: AvailablePortsPool, quarantined_ports: QuarantinedPorts, cooldown: Duration) {
    info!("Port karantina temizleme görevi başlatıldı. Soğuma süresi: {:?}", cooldown);
    let mut interval = tokio::time::interval(cooldown);
    loop {
        interval.tick().await;
        let mut quarantined_guard = quarantined_ports.lock().await;
        if quarantined_guard.is_empty() { continue; }

        let now = Instant::now();
        let mut available_ports_guard = available_ports.lock().await;
        
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

async fn shutdown_signal() {
    let ctrl_c = async { tokio::signal::ctrl_c().await.expect("Failed to install Ctrl+C handler"); };
    #[cfg(unix)] let terminate = async { tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("Failed to install signal handler").recv().await; };
    #[cfg(not(unix))] let terminate = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = terminate => {}, }
    info!("Kapatma sinyali alındı. Graceful shutdown başlatılıyor...");
}

// --- GRPC TRAIT IMPLEMENTATION ---
#[tonic::async_trait]
impl MediaService for MyMediaService {
    #[instrument(skip(self, _request), fields(call_id = %_request.get_ref().call_id))]
    async fn allocate_port(&self, _request: Request<AllocatePortRequest>) -> Result<Response<AllocatePortResponse>, Status> {
        const MAX_RETRIES: u8 = 5;
        let mut available_ports_guard = self.available_ports.lock().await;

        for i in 0..MAX_RETRIES {
            let port_to_try = match available_ports_guard.pop() {
                Some(p) => p,
                None => {
                    warn!("Deneme sırasında port havuzu tükendi.");
                    if i < MAX_RETRIES - 1 {
                        drop(available_ports_guard);
                        sleep(Duration::from_millis(100)).await;
                        available_ports_guard = self.available_ports.lock().await;
                        continue;
                    }
                    return Err(Status::resource_exhausted("Tüm denemelere rağmen uygun port bulunamadı"));
                }
            };
            
            match UdpSocket::bind(format!("{}:{}", self.config.rtp_host, port_to_try)).await {
                Ok(socket) => {
                    info!(port = port_to_try, "Port başarıyla bağlandı.");
                    let (tx, rx) = mpsc::channel(10);
                    self.session_channels.lock().await.insert(port_to_try, tx);
                    
                    tokio::spawn(rtp_session_handler(
                        Arc::new(socket), rx, self.session_channels.clone(), self.quarantined_ports.clone(), 
                        self.audio_cache.clone(), self.config.clone(), port_to_try
                    ));
                    return Ok(Response::new(AllocatePortResponse { rtp_port: port_to_try as u32 }));
                },
                Err(e) => {
                    warn!(port = port_to_try, error = %e, "Porta bağlanılamadı, karantinaya alınıp başka port denenecek.");
                    self.quarantined_ports.lock().await.push((port_to_try, Instant::now()));
                    continue;
                }
            }
        }
        Err(Status::resource_exhausted("Tüm denemelere rağmen uygun port bulunamadı"))
    }
    
    #[instrument(skip(self), fields(port = %request.get_ref().rtp_port))]
    async fn release_port(&self, request: Request<ReleasePortRequest>) -> Result<Response<ReleasePortResponse>, Status> {
        let port = request.into_inner().rtp_port as u16;
        if let Some(tx) = self.session_channels.lock().await.get(&port) {
            info!(port, "Oturum sonlandırma sinyali gönderiliyor.");
            if tx.send(RtpCommand::Shutdown).await.is_err() {
                 warn!(port, "Shutdown komutu gönderilemedi (kanal kapalı).");
            }
        } else {
            warn!(port, "Serbest bırakılacak oturum bulunamadı.");
        }
        Ok(Response::new(ReleasePortResponse { success: true }))
    }
    
    #[instrument(skip(self, request), fields(port = %request.get_ref().server_rtp_port, audio_id = %request.get_ref().audio_id))]
    async fn play_audio(&self, request: Request<PlayAudioRequest>) -> Result<Response<PlayAudioResponse>, Status> {
        let req = request.into_inner();
        let rtp_port = req.server_rtp_port as u16;
        if let Some(tx) = self.session_channels.lock().await.get(&rtp_port) {
            let command = RtpCommand::PlayFile { 
                audio_id: req.audio_id, 
                candidate_target_addr: req.rtp_target_addr.parse().map_err(|_| Status::invalid_argument("Geçersiz hedef adres"))? 
            };
            tx.send(command).await.map_err(|_| Status::internal("RTP oturum kanalı kapalı."))?;
            info!(port = rtp_port, "PlayAudio komutu sıraya alındı.");
            Ok(Response::new(PlayAudioResponse { success: true, message: "Playback queued".to_string() }))
        } else {
            Err(Status::not_found("Belirtilen porta ait aktif oturum yok."))
        }
    }
}

// --- RTP SESSION LOGIC ---
#[instrument(skip_all, fields(rtp_port = port))]
async fn rtp_session_handler(
    socket: Arc<UdpSocket>, mut rx: mpsc::Receiver<RtpCommand>, session_channels: ActiveSessionChannels, 
    quarantined_ports: QuarantinedPorts, audio_cache: AudioCache, config: Arc<AppConfig>, port: u16
) {
    info!("Yeni RTP oturumu dinleyicisi başlatıldı.");
    let mut actual_remote_addr: Option<SocketAddr> = None;
    let mut buf = [0u8; 2048];
    loop {
        tokio::select! {
            biased;
            Some(command) = rx.recv() => match command {
                RtpCommand::PlayFile { audio_id, candidate_target_addr } => {
                    let target = actual_remote_addr.unwrap_or(candidate_target_addr);
                    tokio::spawn(send_announcement(socket.clone(), target, audio_id, audio_cache.clone(), config.clone()));
                },
                RtpCommand::Shutdown => { info!("Shutdown komutu alındı, oturum sonlandırılıyor."); break; }
            },
            result = socket.recv_from(&mut buf) => if let Ok((len, addr)) = result {
                if len > 0 && actual_remote_addr.is_none() {
                    info!(remote = %addr, "İlk RTP paketi alındı, hedef adres doğrulandı.");
                    actual_remote_addr = Some(addr);
                }
            },
        }
    }
    info!("RTP oturumu temizleniyor...");
    session_channels.lock().await.remove(&port);
    quarantined_ports.lock().await.push((port, Instant::now()));
    info!(port, "Port karantinaya alındı.");
}

#[instrument(skip_all, fields(remote = %target_addr, file = %audio_id))]
async fn send_announcement(sock: Arc<UdpSocket>, target_addr: SocketAddr, audio_id: String, cache: AudioCache, config: Arc<AppConfig>) {
    info!("Anons gönderimi başlıyor...");
    if let Err(e) = send_announcement_internal(sock, target_addr, &audio_id, cache, config).await {
        error!(error = ?e, "Anons gönderimi sırasında bir hata oluştu.");
    }
}

async fn send_announcement_internal(sock: Arc<UdpSocket>, target_addr: SocketAddr, audio_id: &str, cache: AudioCache, config: Arc<AppConfig>) -> Result<(), Box<dyn Error + Send + Sync>> {
    let samples = {
        let mut cache_guard = cache.lock().await;
        if let Some(cached_samples) = cache_guard.get(audio_id) {
            debug!("Ses dosyası önbellekten okundu.");
            cached_samples.clone()
        } else {
            info!("Ses dosyası diskten okunuyor ve önbelleğe alınıyor.");
            let mut path = PathBuf::from(&config.assets_base_path);
            path.push(audio_id);
            let path_str = path.to_str().ok_or("Geçersiz dosya yolu karakterleri")?;
            debug!(path = path_str, "Oluşturulan dosya yolu.");
            let new_samples = Arc::new(hound::WavReader::open(path)?.samples::<i16>().filter_map(Result::ok).collect::<Vec<i16>>());
            cache_guard.insert(audio_id.to_string(), new_samples.clone());
            new_samples
        }
    };
    
    let ssrc: u32 = rand::thread_rng().gen();
    let mut sequence_number: u16 = rand::thread_rng().gen();
    let mut timestamp: u32 = rand::thread_rng().gen();
    const SAMPLES_PER_PACKET: usize = 160;

    for chunk in samples.chunks(SAMPLES_PER_PACKET) {
        let packet = Packet {
            header: Header { version: 2, payload_type: 0, sequence_number, timestamp, ssrc, ..Default::default() },
            payload: chunk.iter().map(|&s| linear_to_ulaw(s)).collect(),
        };
        sock.send_to(&packet.marshal()?, target_addr).await?;
        sequence_number = sequence_number.wrapping_add(1);
        timestamp = timestamp.wrapping_add(SAMPLES_PER_PACKET as u32);
        sleep(Duration::from_millis(20)).await;
    }
    info!("Anons gönderimi tamamlandı.");
    Ok(())
}

// --- UTILITY - PCM to u-law conversion ---
const BIAS: i16 = 0x84;
// DÜZELTME: Tam, 256 elemanlı array
static ULAW_TABLE: [u8; 256] = [
    0, 0, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3, 3, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4,
    5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5,
    6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6,
    6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6,
    7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
    7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
    7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
    7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7
];
fn linear_to_ulaw(mut pcm_val: i16) -> u8 {
    let sign = if pcm_val < 0 { 0x80 } else { 0 };
    if sign != 0 { pcm_val = -pcm_val; }
    pcm_val = pcm_val.min(32635);
    pcm_val += BIAS;
    let exponent = ULAW_TABLE[((pcm_val >> 7) & 0xFF) as usize];
    let mantissa = (pcm_val >> (exponent as i16 + 3)) & 0xF;
    !(sign as u8 | (exponent << 4) | mantissa as u8)
}