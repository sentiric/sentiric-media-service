// sentiric-media-service/src/grpc/service.rs

use crate::grpc::error::ServiceError;
use crate::metrics::GRPC_REQUESTS_TOTAL;
use crate::rtp::command::{RtpCommand, RecordingSession};
use crate::rtp::session::RtpSession;
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
    StreamAudioToCallRequest, StreamAudioToCallResponse,
};
use std::pin::Pin;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::{wrappers::ReceiverStream, Stream, StreamExt};
use tonic::{Request, Response, Status, Streaming};
use tracing::{debug, field, info, instrument, warn, Span};
use crate::metrics::ACTIVE_SESSIONS;

pub struct MyMediaService {
    app_state: AppState,
    config: Arc<crate::config::AppConfig>,
}

impl MyMediaService {
    pub fn new(config: Arc<crate::config::AppConfig>, app_state: AppState) -> Self {
        Self { app_state, config }
    }
    fn extract_trace_id<T>(req: &Request<T>) -> String {
        req.metadata().get("x-trace-id").and_then(|v| v.to_str().ok()).unwrap_or("unknown").to_string()
    }
}

#[tonic::async_trait]
impl MediaService for MyMediaService {
    
    type StreamAudioToCallStream = Pin<Box<dyn Stream<Item = Result<StreamAudioToCallResponse, Status>> + Send>>;

    #[instrument(skip(self, request), fields(trace_id))]
    async fn stream_audio_to_call(
        &self,
        request: Request<Streaming<StreamAudioToCallRequest>>,
    ) -> Result<Response<Self::StreamAudioToCallStream>, Status> {
        let trace_id = Self::extract_trace_id(&request);
        Span::current().record("trace_id", &trace_id);

        counter!(GRPC_REQUESTS_TOTAL, "method" => "stream_audio_to_call").increment(1);
        info!("StreamAudioToCall isteği başlatıldı.");

        let mut in_stream = request.into_inner();
        
        let first_msg = match in_stream.message().await {
            Ok(Some(msg)) => msg,
            Ok(None) => return Err(Status::invalid_argument("Stream boş")),
            Err(e) => return Err(Status::internal(format!("Stream okuma hatası: {}", e))),
        };

        let call_id = first_msg.call_id.clone();
        if call_id.is_empty() {
            return Err(Status::invalid_argument("Call ID boş olamaz"));
        }

        let session = self.app_state.port_manager.get_session_by_call_id(&call_id).await
            .ok_or_else(|| Status::not_found(format!("Call ID {} için aktif RTP oturumu bulunamadı", call_id)))?;

        let (audio_tx, audio_rx) = mpsc::channel(8192);
        
        session.send_command(RtpCommand::StartOutboundStream { audio_rx }).await
            .map_err(|_| Status::internal("RTP oturumuna komut gönderilemedi"))?;

        let (response_tx, response_rx) = mpsc::channel(1);
        
        tokio::spawn(async move {
            let mut total_bytes = 0;

            if !first_msg.audio_chunk.is_empty() {
                let size = first_msg.audio_chunk.len();
                total_bytes += size;
                info!("🎤 [gRPC-IN] İlk Paket: {} bytes", size);
                if audio_tx.send(first_msg.audio_chunk).await.is_err() {
                    warn!("⚠️ [gRPC-IN] RTP Kanalı kapalı (Early Drop)");
                    return; 
                }
            }

            while let Ok(Some(msg)) = in_stream.message().await {
                if !msg.audio_chunk.is_empty() {
                    let size = msg.audio_chunk.len();
                    total_bytes += size;
                    debug!("🎤 [gRPC-IN] Chunk Alındı: {} bytes", size);
                    
                    if audio_tx.send(msg.audio_chunk).await.is_err() {
                        warn!("⚠️ [gRPC-IN] RTP Kanalı koptu (Session Closed)");
                        break;
                    }
                }
            }
            
            info!("✅ [gRPC-IN] Stream Bitti. Toplam: {} bytes", total_bytes);
            let _ = response_tx.send(Ok(StreamAudioToCallResponse {
                success: true,
                error_message: "".to_string(),
            })).await;
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(response_rx))))
    }
    
    #[instrument(skip_all, fields(service = "media-service", call_id = %request.get_ref().call_id, trace_id))]
    async fn allocate_port(
        &self,
        request: Request<AllocatePortRequest>,
    ) -> Result<Response<AllocatePortResponse>, Status> {
        let trace_id = Self::extract_trace_id(&request);
        Span::current().record("trace_id", &trace_id);

        counter!(GRPC_REQUESTS_TOTAL, "method" => "allocate_port").increment(1);
        let call_id = request.get_ref().call_id.clone();
        info!(call_id = %call_id, "Port tahsis isteği alındı.");

        // DÜZELTME: MAX_RETRIES kaldırıldı. PortManager kendi mekanizmasını kullanıyor.
        let port_to_try = self.app_state.port_manager.get_available_port().await.ok_or_else(|| ServiceError::PortPoolExhausted)?;

        match UdpSocket::bind(format!("{}:{}", self.config.rtp_host, port_to_try)).await {
            Ok(socket) => {
                info!(port = port_to_try, "Port bağlandı, RtpSession nesnesi oluşturuluyor.");
                gauge!(ACTIVE_SESSIONS).increment(1.0);

                let session = RtpSession::new(call_id.clone(), port_to_try, Arc::new(socket), self.app_state.clone());
                self.app_state.port_manager.add_session(port_to_try, session).await;
                
                return Ok(Response::new(AllocatePortResponse {
                    rtp_port: port_to_try as u32,
                }));
            }
            Err(e) => {
                warn!(port = port_to_try, error = %e, "Porta bağlanılamadı, karantinaya alınıyor.");
                self.app_state.port_manager.quarantine_port(port_to_try).await;
                return Err(ServiceError::PortPoolExhausted.into());
            }
        }
    }

    #[instrument(skip(self, request), fields(port = %request.get_ref().rtp_port, trace_id))]
    async fn release_port(&self, request: Request<ReleasePortRequest>) -> Result<Response<ReleasePortResponse>, Status> {
        let trace_id = Self::extract_trace_id(&request);
        Span::current().record("trace_id", &trace_id);

        counter!(GRPC_REQUESTS_TOTAL, "method" => "release_port").increment(1);

        let port = request.into_inner().rtp_port as u16;
        if let Some(session) = self.app_state.port_manager.get_session(port).await {
            info!(port, "Oturum sonlandırma sinyali gönderiliyor.");
            if session.send_command(RtpCommand::Shutdown).await.is_err() {
                warn!(port, "Shutdown komutu gönderilemedi (kanal kapalı).");
            }
        } else {
            warn!(port, "Serbest bırakılacak oturum bulunamadı.");
        }
        Ok(Response::new(ReleasePortResponse { success: true }))
    }

    #[instrument(skip(self, request), fields(
        port = %request.get_ref().server_rtp_port,
        audio_uri = field::Empty,
        audio_uri_scheme = field::Empty,
        trace_id
    ))]
    async fn play_audio(
        &self,
        request: Request<PlayAudioRequest>,
    ) -> Result<Response<PlayAudioResponse>, Status> {
        let trace_id = Self::extract_trace_id(&request);
        let span = Span::current();
        span.record("trace_id", &trace_id);

        counter!(GRPC_REQUESTS_TOTAL, "method" => "play_audio").increment(1);

        let req = request.into_inner();
        
        let scheme = extract_uri_scheme(&req.audio_uri);
        span.record("audio_uri_scheme", scheme);

        if req.audio_uri.starts_with("data:") {
             debug!("PlayAudio: Data URI alındı (Length: {})", req.audio_uri.len());
             span.record("audio_uri", "data:..."); 
        } else {
             debug!("PlayAudio: Dosya URI alındı: {}", req.audio_uri);
             span.record("audio_uri", &req.audio_uri);
        }

        let rtp_port = req.server_rtp_port as u16;
        let session = self.app_state.port_manager.get_session(rtp_port).await
            .ok_or_else(|| ServiceError::SessionNotFound { port: rtp_port })?;
        
        let target_addr = req.rtp_target_addr.parse().map_err(|e| {
            ServiceError::InvalidTargetAddress { addr: req.rtp_target_addr, source: e }
        })?;

        let (responder_tx, responder_rx) = oneshot::channel();
        let command = RtpCommand::PlayAudioUri {
            audio_uri: req.audio_uri,
            candidate_target_addr: target_addr,
            cancellation_token: tokio_util::sync::CancellationToken::new(),
            responder: Some(responder_tx),
        };

        session.send_command(command).await.map_err(|_| ServiceError::CommandSendError("PlayAudioUri".to_string()))?;

        match responder_rx.await {
            Ok(Ok(_)) => Ok(Response::new(PlayAudioResponse { success: true, message: "OK".to_string() })),
            Ok(Err(e)) => Err(Status::internal(e.to_string())),
            Err(_e) => Err(Status::internal("Response channel closed").into()),
        }
    }

    type RecordAudioStream = Pin<Box<dyn Stream<Item = Result<RecordAudioResponse, Status>> + Send>>;

    #[instrument(skip(self, request), fields(port = %request.get_ref().server_rtp_port, trace_id))]
    async fn record_audio(
        &self,
        request: Request<RecordAudioRequest>,
    ) -> Result<Response<Self::RecordAudioStream>, Status> {
        let trace_id = Self::extract_trace_id(&request);
        Span::current().record("trace_id", &trace_id);

        counter!(GRPC_REQUESTS_TOTAL, "method" => "record_audio").increment(1);
        let req = request.into_inner();
        let rtp_port = req.server_rtp_port as u16;
        
        let session = self.app_state.port_manager.get_session(rtp_port).await
            .ok_or_else(|| ServiceError::SessionNotFound { port: rtp_port })?;
            
        let (stream_tx, stream_rx) = mpsc::channel(self.config.live_audio_stream_buffer);
        let command = RtpCommand::StartLiveAudioStream {
            stream_sender: stream_tx,
            target_sample_rate: req.target_sample_rate,
        };
        session.send_command(command).await.map_err(|_| ServiceError::CommandSendError("StartLive".into()))?;
        
        let output_stream = ReceiverStream::new(stream_rx).map(|res| {
            res.map(|frame| RecordAudioResponse {
                audio_data: frame.data.into(),
                media_type: frame.media_type,
            })
        });
        Ok(Response::new(Box::pin(output_stream)))
    }

    #[instrument(skip(self, request), fields(
        port = %request.get_ref().server_rtp_port,
        call_id = %request.get_ref().call_id,
        trace_id
    ))]
    async fn start_recording(
        &self,
        request: Request<StartRecordingRequest>,
    ) -> Result<Response<StartRecordingResponse>, Status> {
        let trace_id = Self::extract_trace_id(&request);
        Span::current().record("trace_id", &trace_id);

        counter!(GRPC_REQUESTS_TOTAL, "method" => "start_recording").increment(1);
        let req_ref = request.get_ref();
        
        let rtp_port = req_ref.server_rtp_port as u16;
        let session = self.app_state.port_manager.get_session(rtp_port).await
            .ok_or_else(|| ServiceError::SessionNotFound { port: rtp_port })?;
            
        let session_data = RecordingSession {
            output_uri: req_ref.output_uri.clone(),
            spec: WavSpec { channels: 1, sample_rate: 8000, bits_per_sample: 16, sample_format: SampleFormat::Int },
            mixed_samples_16khz: Vec::new(),
            call_id: req_ref.call_id.clone(),
            trace_id: req_ref.trace_id.clone(),
        };

        session.send_command(RtpCommand::StartPermanentRecording(session_data)).await
            .map_err(|_| ServiceError::CommandSendError("StartPermanentRecording".to_string()))?;
            
        Ok(Response::new(StartRecordingResponse { success: true }))
    }

    #[instrument(skip(self, request), fields(port = %request.get_ref().server_rtp_port, trace_id))]
    async fn stop_recording(
        &self,
        request: Request<StopRecordingRequest>,
    ) -> Result<Response<StopRecordingResponse>, Status> {
        let trace_id = Self::extract_trace_id(&request);
        Span::current().record("trace_id", &trace_id);

        counter!(GRPC_REQUESTS_TOTAL, "method" => "stop_recording").increment(1);
        let rtp_port = request.get_ref().server_rtp_port as u16;
        let session = self.app_state.port_manager.get_session(rtp_port).await
            .ok_or_else(|| ServiceError::SessionNotFound { port: rtp_port })?;
            
        let (tx, rx) = oneshot::channel();
        session.send_command(RtpCommand::StopPermanentRecording { responder: tx }).await
            .map_err(|_| ServiceError::CommandSendError("StopPermanentRecording".to_string()))?;
            
        match rx.await {
            Ok(Ok(_)) => Ok(Response::new(StopRecordingResponse { success: true })),
            Ok(Err(e)) => Err(ServiceError::RecordingSaveFailed { source: e }.into()),
            Err(_e) => Err(Status::internal("Response channel closed").into()),
        }
    }
}