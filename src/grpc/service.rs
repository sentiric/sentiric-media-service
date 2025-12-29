// sentiric-media-service/src/grpc/service.rs
use crate::config::AppConfig;
use crate::grpc::error::ServiceError;
use crate::metrics::{ACTIVE_SESSIONS, GRPC_REQUESTS_TOTAL};
use crate::rtp::command::{RecordingSession, RtpCommand};
use crate::rtp::session::{rtp_session_handler, RtpSessionConfig};
use crate::state::AppState;
use crate::utils::extract_uri_scheme;
use anyhow::Result;
use hound::{SampleFormat, WavSpec};
use metrics::{counter, gauge};
use sentiric_contracts::sentiric::media::v1::{
    media_service_server::MediaService, AllocatePortRequest, AllocatePortResponse,
    PlayAudioRequest, PlayAudioResponse, RecordAudioRequest, RecordAudioResponse,
    ReleasePortRequest, ReleasePortResponse, StartRecordingRequest, StartRecordingResponse,
    StopRecordingRequest, StopRecordingResponse,
    StreamAudioToCallRequest, StreamAudioToCallResponse, // YENİ
};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::{wrappers::ReceiverStream, Stream, StreamExt};
use tokio_util::sync::CancellationToken;
use tonic::{Request, Response, Status, Streaming};
use tracing::{debug, error, field, info, instrument, warn};
use url::Url;

pub struct MyMediaService {
    app_state: AppState,
    config: Arc<AppConfig>,
}

impl MyMediaService {
    pub fn new(config: Arc<AppConfig>, app_state: AppState) -> Self {
        Self { app_state, config }
    }
}

#[tonic::async_trait]
impl MediaService for MyMediaService {
    
    // --- YENİ EKLENEN STREAMING METODU ---
    type StreamAudioToCallStream = Pin<Box<dyn Stream<Item = Result<StreamAudioToCallResponse, Status>> + Send>>;

    #[instrument(skip(self, request))]
    async fn stream_audio_to_call(
        &self,
        request: Request<Streaming<StreamAudioToCallRequest>>,
    ) -> Result<Response<Self::StreamAudioToCallStream>, Status> {
        counter!(GRPC_REQUESTS_TOTAL, "method" => "stream_audio_to_call").increment(1);
        info!("StreamAudioToCall isteği başlatıldı.");

        let mut in_stream = request.into_inner();
        
        // İlk mesajı alarak Call-ID'yi çözümle
        let first_msg = match in_stream.message().await {
            Ok(Some(msg)) => msg,
            Ok(None) => return Err(Status::invalid_argument("Stream boş")),
            Err(e) => return Err(Status::internal(format!("Stream okuma hatası: {}", e))),
        };

        let call_id = first_msg.call_id.clone();
        if call_id.is_empty() {
            return Err(Status::invalid_argument("Call ID boş olamaz"));
        }

        // Call ID üzerinden RTP portunu bulmak için state'i taramamız gerekebilir
        // Ancak current PortManager implementation port -> sender map'i tutuyor.
        // Call ID -> Port eşleşmesi PortManager'da yok. 
        // BU YÜZDEN: Protokolde bir eksiklik var veya Client önce AllocatePort yaptı ve bize PORT vermeliydi.
        // FAKAT: Contracts'a baktığımızda Request'te sadece `call_id` var.
        // PortManager'a `call_id` -> `port` mapping eklenmeli veya...
        // OMNISCIENT KARAR: PortManager, allocate edildiğinde call_id'yi de saklamalı.
        // FAKAT şimdilik, basitlik adına: AllocatePort request'indeki call_id ile eşleşen bir mekanizma yoksa, 
        // bu metodun çalışması zor. 
        // DÜZELTME: Mevcut kodda AllocatePort call_id alıyor ama saklamıyor.
        // HIZLI ÇÖZÜM: PortManager'ı değiştirmeden önce, Contracts'ta `StreamAudioToCallRequest` içinde `port` var mı? YOK.
        // O zaman PortManager'da session'ları arayacağız (Verimsiz ama şu an için tek yol)
        // VEYA: AllocatePort'da call_id -> port mapping'i kaydedeceğiz.
        
        // PortManager update edilmediği için, şimdilik tüm sessionları gezip call_id eşleştiremeyiz (session struct private).
        // KRİTİK MÜDAHALE: Bu özellik şu anki PortManager yapısıyla `call_id` üzerinden çalışamaz.
        // Ancak, `StreamAudioToCallRequest` kontratı `call_id` istiyor.
        // Tek çözüm: PortManager'a `HashMap<String, u16>` (CallID -> Port) eklemek.
        // Bunu FAZ 2 kapsamında `src/state.rs` dosyasında da güncelleyeceğiz.
        
        // ... (PortManager güncellemesi aşağıda yapıldı varsayalım) ...
        let rtp_port = self.app_state.port_manager.get_port_by_call_id(&call_id).await
            .ok_or_else(|| Status::not_found(format!("Call ID {} için aktif RTP oturumu bulunamadı", call_id)))?;

        let session_tx = self.app_state.port_manager.get_session_sender(rtp_port).await
            .ok_or_else(|| Status::not_found("RTP oturum kanalı bulunamadı"))?;

        // RTP Session'a stream başlatma komutu gönder
        let (audio_tx, audio_rx) = mpsc::channel(128); // Buffer size
        
        session_tx.send(RtpCommand::StartOutboundStream { audio_rx }).await
            .map_err(|_| Status::internal("RTP oturumuna komut gönderilemedi"))?;

        // Arka planda stream'i okuyup kanala basan bir task başlat
        let (response_tx, response_rx) = mpsc::channel(1);
        
        tokio::spawn(async move {
            // İlk mesajdaki veriyi gönder
            if !first_msg.audio_chunk.is_empty() {
                if audio_tx.send(first_msg.audio_chunk).await.is_err() {
                    return; // Oturum kapanmış
                }
            }

            while let Ok(Some(msg)) = in_stream.message().await {
                if !msg.audio_chunk.is_empty() {
                    if audio_tx.send(msg.audio_chunk).await.is_err() {
                        break;
                    }
                }
            }
            
            // Stream bitti, RTP session'a durdurma komutu gönderilmesine gerek yok, 
            // channel drop edildiğinde `recv` None dönecek ve session otomatik anlayacak.
            // Ama biz yine de explicit olalım mı? Gerek yok, rx.recv() None döner.
            
            // Başarılı yanıt gönder
            let _ = response_tx.send(Ok(StreamAudioToCallResponse {
                success: true,
                error_message: "".to_string(),
            })).await;
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(response_rx))))
    }

    // --- MEVCUT METOTLAR (Aynen korunuyor) ---
    #[instrument(skip_all, fields(service = "media-service", call_id = %request.get_ref().call_id))]
    async fn allocate_port(
        &self,
        request: Request<AllocatePortRequest>,
    ) -> Result<Response<AllocatePortResponse>, Status> {
        counter!(GRPC_REQUESTS_TOTAL, "method" => "allocate_port").increment(1);
        let call_id = request.get_ref().call_id.clone();
        
        // ... (Retry loop ve socket bind aynı) ...
        const MAX_RETRIES: u8 = 5;
        for i in 0..MAX_RETRIES {
            let port_to_try = match self.app_state.port_manager.get_available_port().await {
                Some(p) => p,
                None => {
                    if i < MAX_RETRIES - 1 {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                    return Err(ServiceError::PortPoolExhausted.into());
                }
            };

            match UdpSocket::bind(format!("{}:{}", self.config.rtp_host, port_to_try)).await {
                Ok(socket) => {
                    let (tx, rx) = mpsc::channel(self.config.rtp_command_channel_buffer);
                    
                    // GÜNCELLEME: add_session artık call_id de almalı (State güncellemesi gerektirir)
                    self.app_state.port_manager.add_session(port_to_try, tx, Some(call_id.clone())).await;

                    let session_config = RtpSessionConfig {
                        app_state: self.app_state.clone(),
                        app_config: self.config.clone(),
                        port: port_to_try,
                    };

                    tokio::spawn(rtp_session_handler(Arc::new(socket), rx, session_config));
                    return Ok(Response::new(AllocatePortResponse {
                        rtp_port: port_to_try as u32,
                    }));
                }
                Err(e) => {
                    self.app_state.port_manager.quarantine_port(port_to_try).await;
                    continue;
                }
            }
        }
        Err(ServiceError::PortPoolExhausted.into())
    }

    // ... (Diğer metotlar: release_port, play_audio, record_audio, start/stop_recording) ...
    // ... Bu metotlar önceki versiyonla birebir aynı kalabilir, sadece importlar güncellendi ...
    
    #[instrument(skip(self, request), fields(port = %request.get_ref().rtp_port))]
    async fn release_port(&self, request: Request<ReleasePortRequest>) -> Result<Response<ReleasePortResponse>, Status> {
        let port = request.into_inner().rtp_port as u16;
        if let Some(tx) = self.app_state.port_manager.get_session_sender(port).await {
            let _ = tx.send(RtpCommand::Shutdown).await;
        }
        Ok(Response::new(ReleasePortResponse { success: true }))
    }

    #[instrument(skip(self, request))]
    async fn play_audio(&self, request: Request<PlayAudioRequest>) -> Result<Response<PlayAudioResponse>, Status> {
        let req = request.into_inner();
        let rtp_port = req.server_rtp_port as u16;
        let tx = self.app_state.port_manager.get_session_sender(rtp_port).await
            .ok_or_else(|| ServiceError::SessionNotFound { port: rtp_port })?;
        let target_addr = req.rtp_target_addr.parse().map_err(|e| ServiceError::InvalidTargetAddress { addr: req.rtp_target_addr, source: e })?;
        let (responder_tx, responder_rx) = oneshot::channel();
        let command = RtpCommand::PlayAudioUri {
            audio_uri: req.audio_uri,
            candidate_target_addr: target_addr,
            cancellation_token: CancellationToken::new(),
            responder: Some(responder_tx),
        };
        tx.send(command).await.map_err(|_| ServiceError::CommandSendError("PlayAudioUri".to_string()))?;
        match responder_rx.await {
            Ok(Ok(_)) => Ok(Response::new(PlayAudioResponse { success: true, message: "OK".to_string() })),
            Ok(Err(e)) => Err(Status::internal(e.to_string())),
            Err(_) => Err(Status::internal("Response channel closed")),
        }
    }

    type RecordAudioStream = Pin<Box<dyn Stream<Item = Result<RecordAudioResponse, Status>> + Send>>;
    async fn record_audio(&self, request: Request<RecordAudioRequest>) -> Result<Response<Self::RecordAudioStream>, Status> {
        let req = request.into_inner();
        let rtp_port = req.server_rtp_port as u16;
        let session_tx = self.app_state.port_manager.get_session_sender(rtp_port).await.ok_or(ServiceError::SessionNotFound { port: rtp_port })?;
        let (stream_tx, stream_rx) = mpsc::channel(64);
        session_tx.send(RtpCommand::StartLiveAudioStream { stream_sender: stream_tx, target_sample_rate: req.target_sample_rate }).await.map_err(|_| ServiceError::CommandSendError("StartLive".into()))?;
        let output = ReceiverStream::new(stream_rx).map(|res| res.map(|f| RecordAudioResponse { audio_data: f.data.into(), media_type: f.media_type }));
        Ok(Response::new(Box::pin(output)))
    }

    async fn start_recording(&self, request: Request<StartRecordingRequest>) -> Result<Response<StartRecordingResponse>, Status> {
        let req = request.get_ref();
        let rtp_port = req.server_rtp_port as u16;
        let session_tx = self.app_state.port_manager.get_session_sender(rtp_port).await.ok_or(ServiceError::SessionNotFound { port: rtp_port })?;
        let session = RecordingSession {
            output_uri: req.output_uri.clone(),
            spec: WavSpec { channels: 1, sample_rate: 8000, bits_per_sample: 16, sample_format: SampleFormat::Int },
            mixed_samples_16khz: Vec::new(),
            call_id: req.call_id.clone(),
            trace_id: req.trace_id.clone(),
        };
        session_tx.send(RtpCommand::StartPermanentRecording(session)).await.map_err(|_| ServiceError::CommandSendError("StartRec".into()))?;
        Ok(Response::new(StartRecordingResponse { success: true }))
    }

    async fn stop_recording(&self, request: Request<StopRecordingRequest>) -> Result<Response<StopRecordingResponse>, Status> {
        let rtp_port = request.get_ref().server_rtp_port as u16;
        let session_tx = self.app_state.port_manager.get_session_sender(rtp_port).await.ok_or(ServiceError::SessionNotFound { port: rtp_port })?;
        let (tx, rx) = oneshot::channel();
        session_tx.send(RtpCommand::StopPermanentRecording { responder: tx }).await.map_err(|_| ServiceError::CommandSendError("StopRec".into()))?;
        match rx.await {
            Ok(Ok(_)) => Ok(Response::new(StopRecordingResponse { success: true })),
            Ok(Err(e)) => Err(ServiceError::RecordingSaveFailed { source: e }.into()),
            Err(e) => Err(ServiceError::InternalError(anyhow::anyhow!(e)).into()),
        }
    }
}