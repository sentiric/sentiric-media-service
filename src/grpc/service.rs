// sentiric-media-service/src/grpc/service.rs

use crate::config::AppConfig;
use crate::grpc::error::ServiceError;
use crate::metrics::GRPC_REQUESTS_TOTAL;
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
    StreamAudioToCallRequest, StreamAudioToCallResponse,
};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::{wrappers::ReceiverStream, Stream, StreamExt};
use tokio_util::sync::CancellationToken;
use tonic::{Request, Response, Status, Streaming};
// DÜZELTME: 'error' importu kaldırıldı (bu dosyada kullanılmıyor), 'Url' tutuldu.
use tracing::{debug, field, info, instrument, warn, Span};
use url::Url;
use crate::metrics::ACTIVE_SESSIONS;

pub struct MyMediaService {
    app_state: AppState,
    config: Arc<AppConfig>,
}

impl MyMediaService {
    pub fn new(config: Arc<AppConfig>, app_state: AppState) -> Self {
        Self { app_state, config }
    }

    // Helper: Metadata'dan Trace ID'yi çek
    fn extract_trace_id<T>(req: &Request<T>) -> String {
        req.metadata()
            .get("x-trace-id")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown")
            .to_string()
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

        let rtp_port = self.app_state.port_manager.get_port_by_call_id(&call_id).await
            .ok_or_else(|| Status::not_found(format!("Call ID {} için aktif RTP oturumu bulunamadı", call_id)))?;

        let session_tx = self.app_state.port_manager.get_session_sender(rtp_port).await
            .ok_or_else(|| Status::not_found("RTP oturum kanalı bulunamadı"))?;

        let (audio_tx, audio_rx) = mpsc::channel(8192);
        
        session_tx.send(RtpCommand::StartOutboundStream { audio_rx }).await
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

        const MAX_RETRIES: u8 = 5;
        for i in 0..MAX_RETRIES {
            let port_to_try = match self.app_state.port_manager.get_available_port().await {
                Some(p) => p,
                None => {
                    warn!(attempt = i + 1, max_attempts = MAX_RETRIES, "Port havuzu tükendi.");
                    if i < MAX_RETRIES - 1 {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                    return Err(ServiceError::PortPoolExhausted.into());
                }
            };

            match UdpSocket::bind(format!("{}:{}", self.config.rtp_host, port_to_try)).await {
                Ok(socket) => {
                    info!(port = port_to_try, "Port başarıyla bağlandı ve oturum başlatılıyor.");
                    gauge!(ACTIVE_SESSIONS).increment(1.0);
                    let (tx, rx) = mpsc::channel(self.config.rtp_command_channel_buffer);
                    
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
                    warn!(port = port_to_try, error = %e, "Porta bağlanılamadı, karantinaya alınıp başka port denenecek.");
                    self.app_state.port_manager.quarantine_port(port_to_try).await;
                    continue;
                }
            }
        }
        Err(ServiceError::PortPoolExhausted.into())
    }

    #[instrument(skip(self, request), fields(port = %request.get_ref().rtp_port, trace_id))]
    async fn release_port(
        &self,
        request: Request<ReleasePortRequest>,
    ) -> Result<Response<ReleasePortResponse>, Status> {
        let trace_id = Self::extract_trace_id(&request);
        Span::current().record("trace_id", &trace_id);

        counter!(GRPC_REQUESTS_TOTAL, "method" => "release_port").increment(1);

        let port = request.into_inner().rtp_port as u16;
        if let Some(tx) = self.app_state.port_manager.get_session_sender(port).await {
            info!(port, "Oturum sonlandırma sinyali gönderiliyor.");
            if tx.send(RtpCommand::Shutdown).await.is_err() {
                warn!(port, "Shutdown komutu gönderilemedi (kanal kapalı).");
            }
        } else {
            warn!(port, "Serbest bırakılacak oturum bulunamadı.");
        }
        Ok(Response::new(ReleasePortResponse { success: true }))
    }

    #[instrument(skip(self, request), fields(
        port = %request.get_ref().server_rtp_port,
        audio_uri = field::Empty, // Başlangıçta boş
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
        
        // Data URI'ler çok uzun olduğu için loglamada kısaltıyoruz, ve şemayı kaydediyoruz
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
        let tx = self.app_state.port_manager.get_session_sender(rtp_port).await
            .ok_or_else(|| ServiceError::SessionNotFound { port: rtp_port })?;
        
        let target_addr = req.rtp_target_addr.parse().map_err(|e| {
            ServiceError::InvalidTargetAddress { addr: req.rtp_target_addr, source: e }
        })?;

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
        
        let session_tx = self.app_state.port_manager.get_session_sender(rtp_port).await
            .ok_or_else(|| ServiceError::SessionNotFound { port: rtp_port })?;
            
        let (stream_tx, stream_rx) = mpsc::channel(self.config.live_audio_stream_buffer);
        let command = RtpCommand::StartLiveAudioStream {
            stream_sender: stream_tx,
            target_sample_rate: req.target_sample_rate,
        };
        session_tx.send(command).await.map_err(|_| ServiceError::CommandSendError("StartLive".into()))?;
        
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
        trace_id,
        output_scheme = field::Empty 
    ))]
    async fn start_recording(
        &self,
        request: Request<StartRecordingRequest>,
    ) -> Result<Response<StartRecordingResponse>, Status> {
        let trace_id = Self::extract_trace_id(&request);
        let span = Span::current();
        span.record("trace_id", &trace_id);

        counter!(GRPC_REQUESTS_TOTAL, "method" => "start_recording").increment(1);
        let req_ref = request.get_ref();
        
        // DÜZELTME: Url::parse sonucunu kullanarak şemayı kaydet, böylece Url importu boşa gitmez.
        if let Ok(url) = Url::parse(&req_ref.output_uri) {
            span.record("output_scheme", url.scheme());
        } else {
             span.record("output_scheme", "invalid");
        }
        
        let rtp_port = req_ref.server_rtp_port as u16;
        let session_tx = self.app_state.port_manager.get_session_sender(rtp_port).await
            .ok_or_else(|| ServiceError::SessionNotFound { port: rtp_port })?;
            
        let session = RecordingSession {
            output_uri: req_ref.output_uri.clone(),
            spec: WavSpec { channels: 1, sample_rate: 8000, bits_per_sample: 16, sample_format: SampleFormat::Int },
            mixed_samples_16khz: Vec::new(),
            call_id: req_ref.call_id.clone(),
            trace_id: req_ref.trace_id.clone(),
        };

        session_tx.send(RtpCommand::StartPermanentRecording(session)).await
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
        let session_tx = self.app_state.port_manager.get_session_sender(rtp_port).await
            .ok_or_else(|| ServiceError::SessionNotFound { port: rtp_port })?;
            
        let (tx, rx) = oneshot::channel();
        session_tx.send(RtpCommand::StopPermanentRecording { responder: tx }).await
            .map_err(|_| ServiceError::CommandSendError("StopPermanentRecording".to_string()))?;
            
        match rx.await {
            Ok(Ok(_)) => Ok(Response::new(StopRecordingResponse { success: true })),
            Ok(Err(e)) => Err(ServiceError::RecordingSaveFailed { source: e }.into()),
            Err(_e) => Err(ServiceError::InternalError(anyhow::anyhow!("Kanal hatası")).into()),
        }
    }
}