// DOSYA: sentiric-media-service/src/main.rs (PRODUCTION-READY)

use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, Mutex};
use tokio::time::{sleep, Duration};

use bytes::Bytes;
use rand::seq::SliceRandom;
use rand::{thread_rng, Rng};
use tonic::{transport::Server, Request, Response, Status};
use tracing::{error, info, instrument, warn};
use tracing_subscriber::EnvFilter;

use rtp::header::Header;
use rtp::packet::Packet;
use webrtc_util::marshal::Marshal;

use sentiric_contracts::sentiric::media::v1::{
    media_service_server::{MediaService, MediaServiceServer},
    AllocatePortRequest, AllocatePortResponse, PlayAudioRequest, PlayAudioResponse,
    ReleasePortRequest, ReleasePortResponse,
};

#[derive(Debug)]
enum RtpCommand {
    PlayFile {
        audio_id: String,
        candidate_target_addr: SocketAddr,
    },
    Shutdown,
}

type ActiveSessionChannels = Arc<Mutex<HashMap<u16, mpsc::Sender<RtpCommand>>>>;
type AvailablePortsPool = Arc<Mutex<Vec<u16>>>;

#[derive(Debug, Clone)]
struct AppConfig {
    grpc_listen_addr: SocketAddr,
    rtp_host: String,
    rtp_port_min: u16,
    rtp_port_max: u16,
}

impl AppConfig {
    fn load_from_env() -> Result<Self, Box<dyn Error>> {
        // bu satır container zamanında başlatma sorunlarına neden oluyor.
        // dotenv::dotenv().ok();
        
        let grpc_port_str = env::var("INTERNAL_GRPC_PORT_MEDIA")
            .map_err(|e| format!("CRITICAL: INTERNAL_GRPC_PORT_MEDIA bulunamadı: {}", e))?;

        let grpc_port = grpc_port_str.parse::<u16>()
            .map_err(|e| format!("CRITICAL: INTERNAL_GRPC_PORT_MEDIA ('{}') parse edilemedi: {}", grpc_port_str, e))?;
        
        let rtp_host = env::var("RTP_HOST").unwrap_or_else(|_| "0.0.0.0".to_string());

        let rtp_port_min_str = env::var("EXTERNAL_RTP_PORT_MIN")
            .map_err(|e| format!("CRITICAL: EXTERNAL_RTP_PORT_MIN bulunamadı: {}", e))?;

        let rtp_port_min = rtp_port_min_str.parse::<u16>()
            .map_err(|e| format!("CRITICAL: EXTERNAL_RTP_PORT_MIN ('{}') parse edilemedi: {}", rtp_port_min_str, e))?;

        let rtp_port_max_str = env::var("EXTERNAL_RTP_PORT_MAX")
            .map_err(|e| format!("CRITICAL: EXTERNAL_RTP_PORT_MAX bulunamadı: {}", e))?;
        
        let rtp_port_max = rtp_port_max_str.parse::<u16>()
            .map_err(|e| format!("CRITICAL: EXTERNAL_RTP_PORT_MAX ('{}') parse edilemedi: {}", rtp_port_max_str, e))?;

        Ok(AppConfig {
            grpc_listen_addr: format!("0.0.0.0:{}", grpc_port).parse()?,
            rtp_host,
            rtp_port_min,
            rtp_port_max,
        })
    }
}

pub struct MyMediaService {
    session_channels: ActiveSessionChannels,
    available_ports: AvailablePortsPool,
    config: Arc<AppConfig>,
}

impl MyMediaService {
    fn new(config: Arc<AppConfig>) -> Self {
        let mut initial_ports = Vec::new();
        let mut port = config.rtp_port_min;
        while port <= config.rtp_port_max {
            if port % 2 == 0 {
                initial_ports.push(port);
            }
            port = port.saturating_add(1);
        }
        info!(port_count = initial_ports.len(), "Kullanılabilir port havuzu oluşturuldu.");

        Self {
            session_channels: Arc::new(Mutex::new(HashMap::new())),
            available_ports: Arc::new(Mutex::new(initial_ports)),
            config,
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().json().with_env_filter(env_filter).init();

    std::panic::set_hook(Box::new(|panic_info| {
        error!(panic_info = %panic_info, "PROGRAM PANIK YAŞADI!");
    }));

    info!("Konfigürasyon yükleniyor...");
    let config = match AppConfig::load_from_env() {
        Ok(cfg) => Arc::new(cfg),
        Err(e) => {
            error!(error = %e.to_string(), "KRITIK: Konfigürasyon yüklenemedi.");
            std::process::exit(1);
        }
    };
    
    let server_addr = config.grpc_listen_addr;
    let media_service = MyMediaService::new(config.clone());

    tokio::spawn(async move {
        info!(address = %server_addr, "gRPC sunucusu dinlemeye başlıyor...");
        if let Err(e) = Server::builder()
            .add_service(MediaServiceServer::new(media_service))
            .serve(server_addr)
            .await
        {
            error!(error = %e, "gRPC sunucusu bir hatayla durdu!");
        }
    });

    info!("Servis çalışıyor. Kapatmak için CTRL+C'ye basın.");
    tokio::signal::ctrl_c().await?;
    info!("Kapatma sinyali alındı, servis durduruluyor.");

    Ok(())
}

#[tonic::async_trait]
impl MediaService for MyMediaService {
    #[instrument(skip(self, _request), fields(call_id = %_request.get_ref().call_id))]
    async fn allocate_port(&self, _request: Request<AllocatePortRequest>) -> Result<Response<AllocatePortResponse>, Status> {
        let mut available_ports_guard = self.available_ports.lock().await;
        available_ports_guard.shuffle(&mut thread_rng());
        let ports_to_try: Vec<u16> = available_ports_guard.clone();

        if ports_to_try.is_empty() {
            warn!("Kullanılabilir port kalmadı.");
            return Err(Status::resource_exhausted("Uygun RTP portu bulunamadı"));
        }

        for &rtp_port in &ports_to_try {
            let bind_addr = format!("{}:{}", self.config.rtp_host, rtp_port);
            match UdpSocket::bind(&bind_addr).await {
                Ok(socket) => {
                    if let Some(index) = available_ports_guard.iter().position(|&p| p == rtp_port) {
                        available_ports_guard.remove(index);
                    }
                    info!(port = rtp_port, "Boş port havuzdan alındı ve başarıyla bağlandı.");
                    let mut channels_guard = self.session_channels.lock().await;
                    let (tx, rx) = mpsc::channel(10);
                    channels_guard.insert(rtp_port, tx);
                    let socket_arc = Arc::new(socket);
                    tokio::spawn(rtp_session_handler(socket_arc, rx, self.session_channels.clone(), self.available_ports.clone(), rtp_port));
                    return Ok(Response::new(AllocatePortResponse { rtp_port: rtp_port as u32 }));
                },
                Err(e) => {
                    warn!(port = rtp_port, error = %e, "Porta bağlanılamadı, bir sonraki port denenecek.");
                }
            }
        }
        error!(total_ports_tried = ports_to_try.len(), "Tüm portlar denendi ancak hiçbirine bağlanılamadı.");
        Err(Status::resource_exhausted("Uygun RTP portu bulunamadı, tüm denemeler başarısız oldu."))
    }
    
    #[instrument(skip(self), fields(port = %request.get_ref().rtp_port))]
    async fn release_port(&self, request: Request<ReleasePortRequest>) -> Result<Response<ReleasePortResponse>, Status> {
        let port_to_release = request.into_inner().rtp_port as u16;
        let channels_guard = self.session_channels.lock().await;
        if let Some(tx) = channels_guard.get(&port_to_release) {
            info!(port = port_to_release, "Oturum sonlandırma sinyali (Shutdown) gönderiliyor.");
            let _ = tx.send(RtpCommand::Shutdown).await;
        } else {
            warn!(port = port_to_release, "Serbest bırakılacak oturum kanalı bulunamadı. Muhtemelen zaten kapanmış.");
        }
        Ok(Response::new(ReleasePortResponse { success: true }))
    }
    
    #[instrument(skip(self, request), fields(target_addr = %request.get_ref().rtp_target_addr, audio_id = %request.get_ref().audio_id))]
    async fn play_audio(&self, request: Request<PlayAudioRequest>) -> Result<Response<PlayAudioResponse>, Status> {
        let req = request.into_inner();
        let remote_addr = req.rtp_target_addr.parse().map_err(|_| Status::invalid_argument("Geçersiz hedef adres"))?;
        let server_port = req.server_rtp_port as u16;
        let channels_guard = self.session_channels.lock().await;
        if let Some(tx) = channels_guard.get(&server_port) {
            let command = RtpCommand::PlayFile { audio_id: req.audio_id, candidate_target_addr: remote_addr };
            if tx.send(command).await.is_err() { 
                return Err(Status::internal("RTP oturum kanalı kapalı, komut gönderilemedi.")) 
            }
            info!(port = server_port, "PlayAudio komutu başarıyla RTP handler'a gönderildi.");
            Ok(Response::new(PlayAudioResponse { success: true, message: "Playback command queued".to_string() }))
        } else {
            Err(Status::not_found("Belirtilen porta ait aktif bir RTP oturumu yok."))
        }
    }
}

async fn rtp_session_handler(
    socket: Arc<UdpSocket>, 
    mut rx: mpsc::Receiver<RtpCommand>, 
    session_channels: ActiveSessionChannels, 
    available_ports: AvailablePortsPool,
    port: u16
) {
    info!(rtp_port = port, "Yeni RTP oturumu için dinleyici başlatıldı");
    let mut actual_remote_addr: Option<SocketAddr> = None;
    let mut buf = [0u8; 2048];
    loop {
        tokio::select! {
            Some(command) = rx.recv() => {
                match command {
                    RtpCommand::PlayFile { audio_id, candidate_target_addr } => {
                        let final_target = actual_remote_addr.unwrap_or(candidate_target_addr);
                        info!(?final_target, file = %audio_id, "PlayFile komutu alındı, ses çalma başlıyor...");
                        let sock_clone = Arc::clone(&socket);
                        tokio::spawn(async move {
                            if let Err(e) = send_announcement(sock_clone, final_target, &audio_id).await {
                                error!(error = %e, "Anons gönderimi başarısız oldu.");
                            }
                        });
                    },
                    RtpCommand::Shutdown => {
                        info!(rtp_port = port, "Shutdown komutu alındı, oturum sonlandırılıyor.");
                        break;
                    }
                }
            }
            Ok((len, addr)) = socket.recv_from(&mut buf) => {
                if len > 0 && actual_remote_addr.is_none() {
                    info!(remote = %addr, rtp_port = port, "İlk RTP paketi alındı, hedef adres doğrulandı.");
                    actual_remote_addr = Some(addr);
                }
            }
            else => { 
                info!(rtp_port = port, "Kanal kapandı veya bir hata oluştu, oturum sonlandırılıyor.");
                break; 
            }
        }
    }
    session_channels.lock().await.remove(&port);
    sleep(Duration::from_millis(500)).await;
    {
        let mut ports_guard = available_ports.lock().await;
        if !ports_guard.contains(&port) {
            ports_guard.push(port);
            info!(rtp_port = port, "RTP oturumu sonlandı ve port havuza geri eklendi.");
        }
    }
}

async fn send_announcement(sock: Arc<UdpSocket>, target_addr: SocketAddr, audio_id: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
    let audio_path = Path::new("/app").join(audio_id);
    let samples = match read_wav_samples(audio_path.to_str().unwrap()) {
        Ok(s) => s,
        Err(e) => {
            error!(error = %e, path = %audio_path.display(), "WAV dosyası okunamadı.");
            return Err(e);
        }
    };
    info!(remote = %target_addr, file = %audio_id, samples_len = samples.len(), "Anons gönderimi başlıyor...");
    let ssrc: u32 = rand::thread_rng().gen();
    let mut sequence_number: u16 = rand::thread_rng().gen();
    let mut timestamp: u32 = rand::thread_rng().gen();
    const SAMPLES_PER_PACKET: usize = 160;
    for chunk in samples.chunks(SAMPLES_PER_PACKET) {
        let payload: Bytes = chunk.iter().map(|&sample| linear_to_ulaw(sample)).collect();
        let packet = Packet {
            header: Header { version: 2, payload_type: 0, sequence_number, timestamp, ssrc, ..Default::default() },
            payload,
        };
        let raw_packet = packet.marshal()?;
        if let Err(e) = sock.send_to(&raw_packet, target_addr).await {
            error!(error = %e, "RTP paketi gönderilemedi, gönderim durduruluyor.");
            break; 
        }
        sequence_number = sequence_number.wrapping_add(1);
        timestamp = timestamp.wrapping_add(SAMPLES_PER_PACKET as u32);
        sleep(Duration::from_millis(20)).await;
    }
    info!(remote = %target_addr, file = %audio_id, "Anons gönderimi tamamlandı.");
    Ok(())
}

fn read_wav_samples(file_path: &str) -> Result<Vec<i16>, Box<dyn Error + Send + Sync>> {
    let mut reader = hound::WavReader::open(file_path)?;
    let spec = reader.spec();
    if spec.sample_rate != 8000 || spec.channels != 1 {
        return Err(format!("Desteklenmeyen WAV formatı: {} Hz, {} kanal. Sadece 8000 Hz, mono desteklenmektedir.", spec.sample_rate, spec.channels).into());
    }
    Ok(reader.samples::<i16>().filter_map(Result::ok).collect())
}

// ... (dosyanın geri kalanı tamamen aynı) ...

const SIGN_BIT: i16 = 0x80;
const SEG_SHIFT: i16 = 4;
const BIAS: i16 = 0x84;

// --- BU DİZİ TAMAMEN DOĞRUSUYLA DEĞİŞTİRİLDİ ---
static SEARCH_TABLE: [u8; 256] = [
    0, 0, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3, 3,
    4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4,
    5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5,
    5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5,
    6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6,
    6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6,
    6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6,
    6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6,
    7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
    7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
    7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
    7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
    7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
    7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
    7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
    7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7
];

fn linear_to_ulaw(pcm_val: i16) -> u8 {
    // ... (bu fonksiyon aynı) ...
    let mut pcm_val = pcm_val;
    let sign = if pcm_val < 0 { SIGN_BIT } else { 0 };
    if sign != 0 { pcm_val = -pcm_val; }
    if pcm_val > 32635 { pcm_val = 32635; }
    pcm_val += BIAS;
    let exponent = SEARCH_TABLE[((pcm_val >> 7) & 0xFF) as usize];
    let mantissa = (pcm_val >> (exponent as i16 + 3)) & 0xF;
    let ulaw_byte = (sign as u8) | (exponent << SEG_SHIFT as u8) | (mantissa as u8);
    !ulaw_byte
}