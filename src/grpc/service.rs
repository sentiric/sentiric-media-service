// sentiric-media-service/src/grpc/service.rs

use crate::grpc::error::ServiceError;
use crate::metrics::{GRPC_REQUESTS_TOTAL, ACTIVE_SESSIONS};
use crate::rtp::command::{RtpCommand, RecordingSession};
use crate::rtp::session::RtpSession;
use crate::state::AppState;
use anyhow::Result;
use hound::{WavSpec, SampleFormat};
use metrics::{counter, gauge};
use sentiric_contracts::sentiric::media::v1::{
    media_service_server::MediaService, AllocatePortRequest, AllocatePortResponse,
    PlayAudioRequest, PlayAudioResponse, RecordAudioRequest, RecordAudioResponse,
    ReleasePortRequest, ReleasePortResponse, StartRecordingRequest, StartRecordingResponse,
    StopRecordingRequest, StopRecordingResponse,
    StreamAudioToCallRequest, StreamAudioToCallResponse,
};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::{wrappers::ReceiverStream, Stream, StreamExt};
use tonic::{Request, Response, Status, Streaming};
use tracing::{info, instrument, Span};

pub struct MyMediaService {
    app_state: AppState,
    config: Arc<crate::config::AppConfig>,
}

impl MyMediaService {
    pub fn new(config: Arc<crate::config::AppConfig>, app_state: AppState) -> Self {
        Self { app_state, config }
    }

    fn extract_trace_id<T>(req: &Request<T>) -> String {
        req.metadata().get("x-trace-id")
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

        let mut in_stream = request.into_inner();
        let first_msg = in_stream.message().await?.ok_or_else(|| Status::invalid_argument("Stream empty"))?;

        let call_id = first_msg.call_id.clone();
        
        // [FIX]: Added .await to resolve the Future before calling ok_or_else
        let session = self.app_state.port_manager.get_session_by_call_id(&call_id).await
            .ok_or_else(|| Status::not_found("Session not found"))?;

        let (audio_tx, audio_rx) = mpsc::channel(8192);
        session.send_command(RtpCommand::StartOutboundStream { audio_rx }).await.map_err(|_| Status::internal("Command fail"))?;

        let (response_tx, response_rx) = mpsc::channel(1);
        tokio::spawn(async move {
            if !first_msg.audio_chunk.is_empty() { let _ = audio_tx.send(first_msg.audio_chunk).await; }
            while let Ok(Some(msg)) = in_stream.message().await {
                if !msg.audio_chunk.is_empty() && audio_tx.send(msg.audio_chunk).await.is_err() { break; }
            }
            let _ = response_tx.send(Ok(StreamAudioToCallResponse { success: true, error_message: "".to_string() })).await;
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(response_rx))))
    }
    
    #[instrument(skip(self, request), fields(call_id = %request.get_ref().call_id, trace_id))]
    async fn allocate_port(&self, request: Request<AllocatePortRequest>) -> Result<Response<AllocatePortResponse>, Status> {
        let _trace_id = Self::extract_trace_id(&request);
        let call_id = request.get_ref().call_id.clone();
        counter!(GRPC_REQUESTS_TOTAL, "method" => "allocate_port").increment(1);

        let port = self.app_state.port_manager.get_available_port().await.ok_or(ServiceError::PortPoolExhausted)?;

        match UdpSocket::bind(format!("{}:{}", self.config.rtp_host, port)).await {
            Ok(socket) => {
                gauge!(ACTIVE_SESSIONS).increment(1.0);
                let session = RtpSession::new(call_id.clone(), port, Arc::new(socket), self.app_state.clone());
                self.app_state.port_manager.add_session(port, session).await;
                Ok(Response::new(AllocatePortResponse { rtp_port: port as u32 }))
            }
            Err(_) => {
                self.app_state.port_manager.quarantine_port(port).await;
                Err(Status::resource_exhausted("Bind failed"))
            }
        }
    }

    #[instrument(skip(self, request), fields(port = %request.get_ref().rtp_port, trace_id))]
    async fn release_port(&self, request: Request<ReleasePortRequest>) -> Result<Response<ReleasePortResponse>, Status> {
        let _trace_id = Self::extract_trace_id(&request);
        let port = request.into_inner().rtp_port as u16;
        if let Some(session) = self.app_state.port_manager.get_session(port).await {
            let _ = session.send_command(RtpCommand::Shutdown).await;
        }
        Ok(Response::new(ReleasePortResponse { success: true }))
    }

    #[instrument(skip(self, request), fields(port = %request.get_ref().server_rtp_port, trace_id))]
    async fn play_audio(&self, request: Request<PlayAudioRequest>) -> Result<Response<PlayAudioResponse>, Status> {
        let _trace_id = Self::extract_trace_id(&request);
        let req = request.into_inner();
        let rtp_port = req.server_rtp_port as u16;
        let session = self.app_state.port_manager.get_session(rtp_port).await.ok_or(ServiceError::SessionNotFound { port: rtp_port })?;

        if req.audio_uri.starts_with("control://") {
            let cmd = req.audio_uri.strip_prefix("control://").unwrap();
            match cmd {
                "enable_echo" => {
                    info!("🔊 Native Echo Reflex ENABLED");
                    let _ = session.send_command(RtpCommand::EnableEchoTest).await;
                    return Ok(Response::new(PlayAudioResponse { success: true, message: "Echo On".into() }));
                }
                "disable_echo" => {
                    let _ = session.send_command(RtpCommand::DisableEchoTest).await;
                    return Ok(Response::new(PlayAudioResponse { success: true, message: "Echo Off".into() }));
                }
                _ => return Err(Status::invalid_argument("Unknown control command"))
            }
        }

        let target_addr: SocketAddr = req.rtp_target_addr.parse().map_err(|e| ServiceError::InvalidTargetAddress { addr: req.rtp_target_addr, source: e })?;
        let _ = session.send_command(RtpCommand::SetTargetAddress { target: target_addr }).await;

        let (tx, rx) = oneshot::channel();
        session.send_command(RtpCommand::PlayAudioUri {
            audio_uri: req.audio_uri,
            candidate_target_addr: target_addr,
            cancellation_token: tokio_util::sync::CancellationToken::new(),
            responder: Some(tx),
        }).await.map_err(|_| Status::internal("Command send fail"))?;

        match rx.await {
            Ok(Ok(_)) => Ok(Response::new(PlayAudioResponse { success: true, message: "OK".into() })),
            _ => Err(Status::internal("Playback failed"))
        }
    }

    type RecordAudioStream = Pin<Box<dyn Stream<Item = Result<RecordAudioResponse, Status>> + Send>>;
    async fn record_audio(&self, request: Request<RecordAudioRequest>) -> Result<Response<Self::RecordAudioStream>, Status> {
        let req = request.into_inner();
        let rtp_port = req.server_rtp_port as u16;
        let session = self.app_state.port_manager.get_session(rtp_port).await.ok_or(ServiceError::SessionNotFound { port: rtp_port })?;
        let (tx, rx) = mpsc::channel(64);
        session.send_command(RtpCommand::StartLiveAudioStream { stream_sender: tx, target_sample_rate: req.target_sample_rate }).await.map_err(|_| Status::internal("Fail"))?;
        let out = ReceiverStream::new(rx).map(|r| r.map(|f| RecordAudioResponse { audio_data: f.data.into(), media_type: f.media_type }));
        Ok(Response::new(Box::pin(out)))
    }

    async fn start_recording(&self, request: Request<StartRecordingRequest>) -> Result<Response<StartRecordingResponse>, Status> {
        let req = request.into_inner();
        let session = self.app_state.port_manager.get_session(req.server_rtp_port as u16).await.ok_or(Status::not_found("No session"))?;
        session.send_command(RtpCommand::StartPermanentRecording(RecordingSession {
            output_uri: req.output_uri,
            spec: WavSpec { channels: 1, sample_rate: 8000, bits_per_sample: 16, sample_format: SampleFormat::Int },
            mixed_samples_16khz: Vec::new(),
            call_id: req.call_id,
            trace_id: req.trace_id,
        })).await.map_err(|_| Status::internal("Command fail"))?;
        Ok(Response::new(StartRecordingResponse { success: true }))
    }

    async fn stop_recording(&self, request: Request<StopRecordingRequest>) -> Result<Response<StopRecordingResponse>, Status> {
        let session = self.app_state.port_manager.get_session(request.into_inner().server_rtp_port as u16).await.ok_or(Status::not_found("No session"))?;
        let (tx, rx) = oneshot::channel();
        session.send_command(RtpCommand::StopPermanentRecording { responder: tx }).await.map_err(|_| Status::internal("Command fail"))?;
        match rx.await { Ok(Ok(_)) => Ok(Response::new(StopRecordingResponse { success: true })), _ => Err(Status::internal("Finalize fail")) }
    }
}