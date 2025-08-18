// ========== FILE: sentiric-media-service/src/grpc/service.rs ==========
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, Mutex};
use tokio_util::sync::CancellationToken;
use tonic::{Request, Response, Status};
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

fn extract_uri_scheme(uri: &str) -> &str {
    if let Some(scheme_end) = uri.find(':') { &uri[..scheme_end] } else { "unknown" }
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
                        Arc::new(socket), rx, self.port_manager.clone(),
                        self.audio_cache.clone(), self.config.clone(), port_to_try,
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
    
    #[instrument(skip(self), fields(port = %request.get_ref().rtp_port))]
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
    
    #[instrument(skip(self, request), fields(
        port = request.get_ref().server_rtp_port,
        uri_scheme = extract_uri_scheme(&request.get_ref().audio_uri)
    ))]
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
        
        // --- YENİ AKIŞ KONTROL MANTIĞI ---
        // 1. Önce mevcut çalmayı durdurmak için bir komut gönder (eğer varsa).
        if tx.send(RtpCommand::StopAudio).await.is_err() {
             error!(port = rtp_port, "StopAudio komutu gönderilemedi, kanal kapalı olabilir.");
             return Err(Status::internal("RTP oturum kanalı kapalı."));
        }

        // 2. Yeni çalma işlemi için yeni bir iptal token'ı oluştur.
        let cancellation_token = CancellationToken::new();
        
        let command = RtpCommand::PlayAudioUri { 
            audio_uri: req.audio_uri, 
            candidate_target_addr: target_addr,
            cancellation_token: cancellation_token.clone(),
        };
        
        if tx.send(command).await.is_err() {
            error!(port = rtp_port, "PlayAudioUri komutu gönderilemedi, kanal kapalı.");
            return Err(Status::internal("RTP oturum kanalı kapalı."));
        }
        info!(port = rtp_port, "PlayAudio komutu sıraya alındı.");

        // `agent-service`'in beklemesini sağlamak için, ses çalma işlemi iptal edilene kadar bekleyeceğiz.
        // Bu, `send_announcement_from_uri` içinden anons bitince veya dışarıdan yeni komut gelince tetiklenir.
        cancellation_token.cancelled().await;
        info!(port = rtp_port, "Anons çalma işlemi tamamlandı veya yeni bir komutla iptal edildi.");
            
        Ok(Response::new(PlayAudioResponse { success: true, message: "Playback completed or was interrupted.".to_string() }))
    }
}