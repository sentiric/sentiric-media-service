use std::collections::HashSet;
use std::env;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::time::{sleep, Duration};

use async_mutex::Mutex;
use bytes::Bytes;
use rand::{thread_rng, Rng};
use tonic::{transport::Server, Request, Response, Status};
use tracing::{debug, error, info, instrument};
use tracing_subscriber::EnvFilter;

use rtp::packet::Packet;
use rtp::header::Header;
use webrtc_util::marshal::Marshal;

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
    async fn allocate_port(&self, request: Request<AllocatePortRequest>) -> Result<Response<AllocatePortResponse>, Status> {
        let call_id = &request.get_ref().call_id;
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
                    return Ok(Response::new(AllocatePortResponse { rtp_port: rtp_port as u32, }));
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

    #[instrument(skip(self, request))]
    async fn play_audio(&self, request: Request<PlayAudioRequest>) -> Result<Response<PlayAudioResponse>, Status> {
        let req = request.into_inner();
        info!(call_id = %req.call_id, audio_id = %req.audio_id, "PlayAudio isteği alındı.");
        
        let remote_addr_str = "127.0.0.1:4002"; // Test için hedef port
        let remote_addr: SocketAddr = remote_addr_str.parse().map_err(|e| Status::internal(format!("Geçersiz hedef adres: {}", e)))?;

        let samples = match read_wav_samples(&req.audio_id) {
            Ok(s) => s,
            Err(e) => {
                error!(error = %e, "WAV dosyası okunamadı.");
                return Err(Status::internal("Ses dosyası okunamadı."));
            }
        };

        // RASTGELE SAYILARI BURADA, ANA THREAD'DE ÜRETİYORUZ
        let mut rng = thread_rng();
        let ssrc: u32 = rng.gen();
        let sequence_number: u16 = rng.gen();
        let timestamp: u32 = rng.gen();

        // YENİ THREAD'E SADECE GÜVENLİ VERİLERİ (sayılar, vektör) TAŞIYORUZ
        tokio::spawn(async move {
            if let Err(e) = play_samples_to_rtp(remote_addr, samples, ssrc, sequence_number, timestamp).await {
                error!(error = %e, "Ses çalma işlemi başarısız oldu.");
            }
        });

        Ok(Response::new(PlayAudioResponse { success: true }))
    }
}

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
    if spec.sample_rate != 8000 {
        return Err(format!("Desteklenmeyen WAV sample rate: {}", spec.sample_rate).into());
    }
    Ok(reader.samples::<i16>().filter_map(Result::ok).collect())
}

async fn play_samples_to_rtp(dest: SocketAddr, samples: Vec<i16>, ssrc: u32, initial_sequence: u16, initial_timestamp: u32) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("Sample verileri RTP olarak gönderiliyor...");
    
    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    let mut sequence_number = initial_sequence;
    let mut timestamp = initial_timestamp;
    const SAMPLES_PER_PACKET: usize = 160;

    for chunk in samples.chunks(SAMPLES_PER_PACKET) {
        let payload: Bytes = chunk.iter().map(|&sample| linear_to_ulaw(sample)).collect();
        let packet = Packet {
            header: Header {
                version: 2,
                payload_type: 0,
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
    info!("Sample gönderimi tamamlandı.");
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

const SIGN_BIT: i16 = 0x80;
// const QUANT_MASK: i16 = 0xf; // <-- BU SATIRI SİL
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