// File: src/grpc/service.rs
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio_stream::{Stream, wrappers::ReceiverStream, StreamExt};
use tokio_util::sync::CancellationToken;
use tonic::{Request, Response, Status};
use tracing::{error, info, instrument, warn};

use sentiric_contracts::sentiric::media::v1::RecordAudioResponse;

use crate::{
    AllocatePortRequest, AllocatePortResponse, MediaService, PlayAudioRequest, PlayAudioResponse,
    ReleasePortRequest, ReleasePortResponse, RecordAudioRequest,
};
use crate::config::AppConfig;
use crate::rtp::command::RtpCommand;
use crate::rtp::session::rtp_session_handler;
// YENİ: AppState'i import ediyoruz
use crate::state::AppState;

pub struct MyMediaService {
    // DEĞİŞİKLİK: Ayrı ayrı durumlar yerine tek bir AppState tutuyoruz.
    app_state: AppState,
    config: Arc<AppConfig>,
}

impl MyMediaService {
    pub fn new(config: Arc<AppConfig>, app_state: AppState) -> Self {
        Self { app_state, config }
    }
}
fn extract_uri_scheme(uri: &str) -> &str { if let Some(scheme_end) = uri.find(':') { &uri[..scheme_end] } else { "unknown" } }

#[tonic::async_trait]
impl MediaService for MyMediaService {
    #[instrument(skip(self, _request), fields(call_id = %_request.get_ref().call_id))]
    async fn allocate_port(&self, _request: Request<AllocatePortRequest>) -> Result<Response<AllocatePortResponse>, Status> {
        const MAX_RETRIES: u8 = 5;
        for i in 0..MAX_RETRIES {
            // DEĞİŞİKLİK: self.app_state.port_manager üzerinden erişim
            let port_to_try = match self.app_state.port_manager.get_available_port().await {
                Some(p) => p,
                None => {
                    warn!(attempt = i + 1, max_attempts = MAX_RETRIES, "Port havuzu tükendi.");
                    if i < MAX_RETRIES - 1 { tokio::time::sleep(Duration::from_millis(100)).await; continue; }
                    error!("Tüm denemelere rağmen uygun port bulunamadı.");
                    return Err(Status::resource_exhausted("Tüm denemelere rağmen uygun port bulunamadı"));
                }
            };
            match UdpSocket::bind(format!("{}:{}", self.config.rtp_host, port_to_try)).await {
                Ok(socket) => {
                    info!(port = port_to_try, "Port başarıyla bağlandı ve oturum başlatılıyor.");
                    let (tx, rx) = mpsc::channel(10);
                    // DEĞİŞİKLİK: self.app_state.port_manager üzerinden erişim
                    self.app_state.port_manager.add_session(port_to_try, tx).await;
                    tokio::spawn(rtp_session_handler(
                        Arc::new(socket), 
                        rx, 
                        self.app_state.port_manager.clone(), 
                        // DEĞİŞİKLİK: app_state üzerinden audio_cache'e erişim
                        self.app_state.audio_cache.clone(), 
                        self.config.clone(), 
                        port_to_try,
                    ));
                    return Ok(Response::new(AllocatePortResponse { rtp_port: port_to_try as u32 }));
                },
                Err(e) => {
                    warn!(port = port_to_try, error = %e, "Porta bağlanılamadı, karantinaya alınıp başka port denenecek.");
                    // DEĞİŞİKLİK: self.app_state.port_manager üzerinden erişim
                    self.app_state.port_manager.quarantine_port(port_to_try).await;
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
        // DEĞİŞİKLİK: self.app_state.port_manager üzerinden erişim
        if let Some(tx) = self.app_state.port_manager.get_session_sender(port).await {
            info!(port, "Oturum sonlandırma sinyali gönderiliyor.");
            if tx.send(RtpCommand::Shutdown).await.is_err() { warn!(port, "Shutdown komutu gönderilemedi (kanal zaten kapalı olabilir)."); }
        } else { warn!(port, "Serbest bırakılacak oturum bulunamadı veya çoktan kapatılmış."); }
        Ok(Response::new(ReleasePortResponse { success: true }))
    }
    
    #[instrument(skip(self, request), fields(port = request.get_ref().server_rtp_port, uri_scheme = extract_uri_scheme(&request.get_ref().audio_uri)))]
    async fn play_audio(&self, request: Request<PlayAudioRequest>) -> Result<Response<PlayAudioResponse>, Status> {
        let req = request.into_inner();
        let rtp_port = req.server_rtp_port as u16;
        // DEĞİŞİKLİK: self.app_state.port_manager üzerinden erişim
        let tx = self.app_state.port_manager.get_session_sender(rtp_port).await.ok_or_else(|| Status::not_found(format!("Belirtilen porta ({}) ait aktif oturum yok.", rtp_port)))?;
        let target_addr = req.rtp_target_addr.parse().map_err(|e| { error!(error = %e, addr = %req.rtp_target_addr, "Geçersiz hedef adres formatı."); Status::invalid_argument("Geçersiz hedef adres formatı") })?;
        if tx.send(RtpCommand::StopAudio).await.is_err() { error!(port = rtp_port, "StopAudio komutu gönderilemedi, kanal kapalı olabilir."); return Err(Status::internal("RTP oturum kanalı kapalı.")); }
        let cancellation_token = CancellationToken::new();
        let command = RtpCommand::PlayAudioUri { audio_uri: req.audio_uri, candidate_target_addr: target_addr, cancellation_token: cancellation_token.clone(), };
        if tx.send(command).await.is_err() { error!(port = rtp_port, "PlayAudioUri komutu gönderilemedi, kanal kapalı."); return Err(Status::internal("RTP oturum kanalı kapalı.")); }
        info!(port = rtp_port, "PlayAudio komutu sıraya alındı.");
        cancellation_token.cancelled().await;
        info!(port = rtp_port, "Anons çalma işlemi tamamlandı veya yeni bir komutla iptal edildi.");
        Ok(Response::new(PlayAudioResponse { success: true, message: "Playback completed or was interrupted.".to_string() }))
    }

    type RecordAudioStream = Pin<Box<dyn Stream<Item = Result<RecordAudioResponse, Status>> + Send>>;
    #[instrument(skip(self, request), fields(port = %request.get_ref().server_rtp_port))]
    async fn record_audio(&self, request: Request<RecordAudioRequest>) -> Result<Response<Self::RecordAudioStream>, Status> {
        let req = request.into_inner();
        let rtp_port = req.server_rtp_port as u16;
        // DEĞİŞİKLİK: self.app_state.port_manager üzerinden erişim
        let session_tx = self.app_state.port_manager.get_session_sender(rtp_port).await
            .ok_or_else(|| Status::not_found(format!("Oturum bulunamadı: {}", rtp_port)))?;
        
        let (stream_tx, stream_rx) = mpsc::channel(32);
        
        let command = RtpCommand::StartRecording {
            stream_sender: stream_tx,
            target_sample_rate: req.target_sample_rate,
        };

        session_tx.send(command).await.map_err(|_| Status::internal("Kayıt başlatma komutu gönderilemedi."))?;
        info!("Ses kaydı stream'i başlatıldı.");
        
        let output_stream = ReceiverStream::new(stream_rx).map(|res| {
            res.map(|frame| RecordAudioResponse {
                audio_data: frame.data.into(),
                media_type: frame.media_type,
            })
        });

        Ok(Response::new(Box::pin(output_stream)))
    }
}