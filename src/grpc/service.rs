use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, Mutex};
use tonic::{Request, Response, Status};

// DÜZELTME: Gereksiz ve yinelenen `use` satırını kaldırdık.
// Sadece en kapsamlı olan bu satır kalacak.
use tracing::{error, info, instrument, warn};

use crate::{
    AllocatePortRequest, AllocatePortResponse, MediaService, PlayAudioRequest, PlayAudioResponse,
    ReleasePortRequest, ReleasePortResponse,
};
use crate::audio::AudioCache;
use crate::config::AppConfig;
use crate::rtp::command::RtpCommand;
use crate::rtp::session::rtp_session_handler;
use crate::state::PortManager;

// ... (dosyanın geri kalanında HİÇBİR DEĞİŞİKLİK YOK, olduğu gibi kalacak) ...
pub struct MyMediaService {
    port_manager: PortManager,
    config: Arc<AppConfig>,
    audio_cache: AudioCache,
}

impl MyMediaService {
    pub fn new(config: Arc<AppConfig>, port_manager: PortManager) -> Self {
        Self {
            port_manager,
            config,
            audio_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}


#[tonic::async_trait]
impl MediaService for MyMediaService {
    #[instrument(skip(self, _request), fields(call_id = %_request.get_ref().call_id))]
    async fn allocate_port(&self, _request: Request<AllocatePortRequest>) -> Result<Response<AllocatePortResponse>, Status> {
        const MAX_RETRIES: u8 = 5;
        for i in 0..MAX_RETRIES {
            let port_to_try = match self.port_manager.get_available_port().await {
                Some(p) => p,
                None => {
                    warn!(attempt = i + 1, max_attempts = MAX_RETRIES, "Port havuzu tükendi.");
                    if i < MAX_RETRIES - 1 {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                    error!("Tüm denemelere rağmen uygun port bulunamadı.");
                    return Err(Status::resource_exhausted("Tüm denemelere rağmen uygun port bulunamadı"));
                }
            };

            match UdpSocket::bind(format!("{}:{}", self.config.rtp_host, port_to_try)).await {
                Ok(socket) => {
                    info!(port = port_to_try, "Port başarıyla bağlandı ve oturum başlatılıyor.");
                    let (tx, rx) = mpsc::channel(10);
                    self.port_manager.add_session(port_to_try, tx).await;
                    
                    tokio::spawn(rtp_session_handler(
                        Arc::new(socket),
                        rx,
                        self.port_manager.clone(),
                        self.audio_cache.clone(),
                        self.config.clone(),
                        port_to_try,
                    ));
                    return Ok(Response::new(AllocatePortResponse { rtp_port: port_to_try as u32 }));
                },
                Err(e) => {
                    warn!(port = port_to_try, error = %e, "Porta bağlanılamadı, karantinaya alınıp başka port denenecek.");
                    self.port_manager.quarantine_port(port_to_try).await;
                    continue;
                }
            }
        }
        error!("Tüm denemelerden sonra uygun bir port bulunamadı.");
        Err(Status::resource_exhausted("Tüm denemelere rağmen uygun port bulunamadı"))
    }
    
    // #[instrument(skip(self), fields(port = %request.get_ref().rtp_port))]
    async fn release_port(&self, request: Request<ReleasePortRequest>) -> Result<Response<ReleasePortResponse>, Status> {
        let port = request.into_inner().rtp_port as u16;
        if let Some(tx) = self.port_manager.get_session_sender(port).await {
            info!(port, "Oturum sonlandırma sinyali gönderiliyor.");
            if tx.send(RtpCommand::Shutdown).await.is_err() {
                 warn!(port, "Shutdown komutu gönderilemedi (kanal zaten kapalı olabilir).");
            }
        } else {
            warn!(port, "Serbest bırakılacak oturum bulunamadı veya çoktan kapatılmış.");
        }
        Ok(Response::new(ReleasePortResponse { success: true }))
    }
    
    #[instrument(skip(self, request), fields(port = %request.get_ref().server_rtp_port, audio_id = %request.get_ref().audio_id, rtp_target = %request.get_ref().rtp_target_addr))]
    async fn play_audio(&self, request: Request<PlayAudioRequest>) -> Result<Response<PlayAudioResponse>, Status> {
        let req = request.into_inner();
        let rtp_port = req.server_rtp_port as u16;
        
        let tx = self.port_manager.get_session_sender(rtp_port).await
            .ok_or_else(|| Status::not_found(format!("Belirtilen porta ({}) ait aktif oturum yok.", rtp_port)))?;

        let target_addr = req.rtp_target_addr.parse()
            .map_err(|e| {
                error!(error = %e, addr = %req.rtp_target_addr, "Geçersiz hedef adres formatı.");
                Status::invalid_argument("Geçersiz hedef adres formatı")
            })?;
        
        let command = RtpCommand::PlayFile { 
            audio_id: req.audio_id, 
            candidate_target_addr: target_addr,
        };
        
        tx.send(command).await
            .map_err(|e| {
                error!(error = %e, port = rtp_port, "RTP oturum kanalı kapalı.");
                Status::internal("RTP oturum kanalı kapalı.")
            })?;
            
        info!(port = rtp_port, "PlayAudio komutu sıraya alındı.");
        Ok(Response::new(PlayAudioResponse { success: true, message: "Playback queued".to_string() }))
    }
}