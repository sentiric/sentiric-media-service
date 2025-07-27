// DOSYA: sentiric-media-service/src/main.rs

use std::collections::HashSet;
use std::env;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::time::{sleep, Duration};

use async_mutex::Mutex;
use bytes::Bytes;
use rand::{thread_rng, Rng};
use tonic::{transport::Server, Request, Response, Status};
use tracing::{debug, error, info, instrument, warn};
use tracing_subscriber::EnvFilter;

use rtp::header::Header;
use rtp::packet::Packet;
use webrtc_util::marshal::Marshal;

// --- DEĞİŞİKLİK: Doğru kontratları import ediyoruz ---
use sentiric_contracts::sentiric::media::v1::{
    media_service_server::{MediaService, MediaServiceServer},
    AllocatePortRequest, AllocatePortResponse, PlayAudioRequest, PlayAudioResponse,
    ReleasePortRequest, ReleasePortResponse,
};

type PortPool = Arc<Mutex<HashSet<u16>>>;

#[derive(Debug, Clone)]
struct AppConfig {
    grpc_listen_addr: SocketAddr,
    rtp_host: String,
    rtp_port_min: u16,
    rtp_port_max: u16,
}

// DOSYA: sentiric-media-service/src/main.rs (SADECE BU FONKSİYONU GÜNCELLEYİN)

impl AppConfig {
    fn load_from_env() -> Result<Self, Box<dyn std::error::Error>> {
        dotenv::dotenv().ok();
        
        // --- DEĞİŞİKLİK: Değişken isimlerini ana .env dosyasıyla TAM UYUMLU hale getiriyoruz. ---
        let grpc_port_str = env::var("INTERNAL_GRPC_PORT_MEDIA")
            .expect("INTERNAL_GRPC_PORT_MEDIA ortam değişkeni bulunamadı.");
        let grpc_port = grpc_port_str.parse::<u16>()?;
        
        let rtp_host = env::var("RTP_HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
        
        let rtp_port_min_str = env::var("EXTERNAL_RTP_PORT_MIN")
            .expect("EXTERNAL_RTP_PORT_MIN ortam değişkeni bulunamadı.");
        let rtp_port_max_str = env::var("EXTERNAL_RTP_PORT_MAX")
            .expect("EXTERNAL_RTP_PORT_MAX ortam değişkeni bulunamadı.");

        Ok(AppConfig {
            grpc_listen_addr: format!("0.0.0.0:{}", grpc_port).parse()?,
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
    #[instrument(skip(self, request), fields(call_id = %request.get_ref().call_id))]
    async fn allocate_port(
        &self,
        request: Request<AllocatePortRequest>,
    ) -> Result<Response<AllocatePortResponse>, Status> {
        info!("AllocatePort isteği alındı.");
        let mut ports_guard = self.allocated_ports.lock().await;

        if ports_guard.len() >= (self.config.rtp_port_max - self.config.rtp_port_min) as usize {
            error!("Tüm RTP portları dolu.");
            return Err(Status::resource_exhausted("Available RTP port pool is exhausted."));
        }

        for _ in 0..100 { // Sonsuz döngü riskine karşı deneme sayısını limitliyoruz
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

        error!("100 denemeye rağmen uygun RTP portu bulunamadı.");
        Err(Status::resource_exhausted("Could not find an available RTP port after 100 attempts."))
    }

    #[instrument(skip(self), fields(port = %request.get_ref().rtp_port))]
    async fn release_port(
        &self,
        request: Request<ReleasePortRequest>,
    ) -> Result<Response<ReleasePortResponse>, Status> {
        info!("ReleasePort isteği alındı.");
        let port_to_release = request.into_inner().rtp_port as u16;
        let mut ports_guard = self.allocated_ports.lock().await;

        if ports_guard.remove(&port_to_release) {
            info!(port = port_to_release, "Port başarıyla serbest bırakıldı.");
            Ok(Response::new(ReleasePortResponse { success: true }))
        } else {
            warn!(port = port_to_release, "Serbest bırakılacak port listede bulunamadı.");
            Ok(Response::new(ReleasePortResponse { success: false }))
        }
    }

    // --- DEĞİŞİKLİK: PlayAudio fonksiyonu yeni kontrata göre güncellendi ---
    #[instrument(skip(self, request), fields(target_addr = %request.get_ref().rtp_target_addr, audio_id = %request.get_ref().audio_id))]
    async fn play_audio(
        &self,
        request: Request<PlayAudioRequest>,
    ) -> Result<Response<PlayAudioResponse>, Status> {
        let req = request.into_inner();
        info!("PlayAudio isteği alındı.");

        // `rtp_target_addr` string'ini SocketAddr'a çeviriyoruz.
        let remote_addr: SocketAddr = req.rtp_target_addr.parse().map_err(|e| {
            error!(error = %e, addr = %req.rtp_target_addr, "Geçersiz hedef RTP adresi.");
            Status::invalid_argument(format!("Geçersiz hedef adres: {}", e))
        })?;

        // Docker imajı içindeki path
        let audio_path = Path::new("/app").join(&req.audio_id);

        let samples = match read_wav_samples(audio_path.to_str().unwrap()) {
            Ok(s) => s,
            Err(e) => {
                error!(error = %e, path = %audio_path.display(), "WAV dosyası okunamadı.");
                return Err(Status::internal(format!("Ses dosyası okunamadı: {}", audio_path.display())));
            }
        };

        let mut rng = thread_rng();
        let ssrc: u32 = rng.gen();
        let sequence_number: u16 = rng.gen();
        let timestamp: u32 = rng.gen();

        tokio::spawn(async move {
            info!(target = %remote_addr, "Ses çalma işlemi yeni bir task'te başlatılıyor.");
            if let Err(e) = play_samples_to_rtp(remote_addr, samples, ssrc, sequence_number, timestamp).await {
                error!(error = %e, "Ses çalma işlemi başarısız oldu.");
            }
        });
        
        let success_message = format!("Playback started for target {}", remote_addr);
        Ok(Response::new(PlayAudioResponse { success: true, message: success_message }))
    }
}

// ... (dosyanın geri kalanı aynı kalabilir, aşağıya ekliyorum) ...

async fn handle_rtp_stream(socket: UdpSocket) {
    let local_addr = socket.local_addr().unwrap();
    info!(%local_addr, "Yeni RTP stream için dinleyici başlatıldı.");
    let mut buf = [0; 2048];
    loop {
        if let Ok((len, remote_addr)) = socket.recv_from(&mut buf).await {
            debug!(port = local_addr.port(), %remote_addr, bytes = len, "RTP paketi alındı.");
        }
    }
}

fn read_wav_samples(file_path: &str) -> Result<Vec<i16>, Box<dyn std::error::Error + Send + Sync>> {
    let mut reader = hound::WavReader::open(file_path)?;
    let spec = reader.spec();
    if spec.sample_rate != 8000 || spec.channels != 1 {
        return Err(format!("Desteklenmeyen WAV formatı: {} Hz, {} kanal. Sadece 8000 Hz, mono desteklenmektedir.", spec.sample_rate, spec.channels).into());
    }
    Ok(reader.samples::<i16>().filter_map(Result::ok).collect())
}

async fn play_samples_to_rtp(dest: SocketAddr, samples: Vec<i16>, ssrc: u32, initial_sequence: u16, initial_timestamp: u32) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!(target = %dest, "Sample verileri RTP olarak gönderiliyor...");
    
    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    let mut sequence_number = initial_sequence;
    let mut timestamp = initial_timestamp;
    const SAMPLES_PER_PACKET: usize = 160; // 20ms için 8kHz'de 160 sample

    for chunk in samples.chunks(SAMPLES_PER_PACKET) {
        let payload: Bytes = chunk.iter().map(|&sample| linear_to_ulaw(sample)).collect();
        let packet = Packet {
            header: Header {
                version: 2,
                payload_type: 0, // PCMU için payload type 0'dır
                sequence_number,
                timestamp,
                ssrc,
                ..Default::default()
            },
            payload,
        };
        let raw_packet = packet.marshal()?;
        socket.send_to(&raw_packet, dest).await?;
        sequence_number = sequence_number.wrapping_add(1);
        timestamp = timestamp.wrapping_add(SAMPLES_PER_PACKET as u32);
        sleep(Duration::from_millis(20)).await;
    }
    info!(target=%dest, "Sample gönderimi tamamlandı.");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().json().with_env_filter(env_filter).init();

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

// PCMU (u-law) dönüşümü için sabitler ve fonksiyon
const SIGN_BIT: i16 = 0x80;
const SEG_SHIFT: i16 = 4;
const BIAS: i16 = 0x84;

static SEARCH_TABLE: [u8; 256] = [0, 0, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3, 3, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7];

fn linear_to_ulaw(pcm_val: i16) -> u8 {
    let mut pcm_val = pcm_val;
    let sign = if pcm_val < 0 { SIGN_BIT } else { 0 };
    if sign != 0 {
        pcm_val = -pcm_val;
    }
    if pcm_val > 32635 { pcm_val = 32635; }
    pcm_val += BIAS;

    let exponent = SEARCH_TABLE[((pcm_val >> 7) & 0xFF) as usize];
    let mantissa = (pcm_val >> (exponent as i16 + 3)) & 0xF;
    let ulaw_byte = (sign as u8) | (exponent << SEG_SHIFT as u8) | (mantissa as u8);
    !ulaw_byte
}