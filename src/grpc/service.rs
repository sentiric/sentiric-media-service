// sentiric-media-service/src/grpc/service.rs
use crate::grpc::error::ServiceError;
use crate::metrics::{ACTIVE_SESSIONS, GRPC_REQUESTS_TOTAL};
use crate::rtp::command::{RecordingSession, RtpCommand};
use crate::rtp::session::RtpSession;
use crate::state::AppState;
use anyhow::Result;
use hound::{SampleFormat, WavSpec};
use metrics::{counter, gauge};
use sentiric_contracts::sentiric::media::v1::{
    media_service_server::MediaService, AllocatePortRequest, AllocatePortResponse,
    PlayAudioRequest, PlayAudioResponse, RecordAudioRequest, RecordAudioResponse,
    ReleasePortRequest, ReleasePortResponse, StartRecordingRequest, StartRecordingResponse,
    StopRecordingRequest, StopRecordingResponse, StreamAudioToCallRequest,
    StreamAudioToCallResponse,
};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::{wrappers::ReceiverStream, Stream, StreamExt};
use tonic::{Request, Response, Status, Streaming};
use tracing::{field, info, instrument, warn, Span};

pub struct MyMediaService {
    app_state: AppState,
    config: Arc<crate::config::AppConfig>,
}

impl MyMediaService {
    pub fn new(config: Arc<crate::config::AppConfig>, app_state: AppState) -> Self {
        Self { app_state, config }
    }

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
    type StreamAudioToCallStream =
        Pin<Box<dyn Stream<Item = Result<StreamAudioToCallResponse, Status>> + Send>>;

    #[instrument(skip(self, request), fields(call_id = field::Empty, trace_id))]
    async fn stream_audio_to_call(
        &self,
        request: Request<Streaming<StreamAudioToCallRequest>>,
    ) -> Result<Response<Self::StreamAudioToCallStream>, Status> {
        let trace_id = Self::extract_trace_id(&request);
        Span::current().record("trace_id", &trace_id);

        let mut in_stream = request.into_inner();
        let first_msg = in_stream
            .message()
            .await?
            .ok_or_else(|| Status::invalid_argument("Stream empty"))?;

        let call_id = first_msg.call_id.clone();
        Span::current().record("call_id", &call_id);

        counter!(GRPC_REQUESTS_TOTAL, "method" => "stream_audio_to_call").increment(1);

        let session = self
            .app_state
            .port_manager
            .get_session_by_call_id(&call_id)
            .await
            .ok_or_else(|| Status::not_found("Session not found"))?;

        let egress_tx = session.egress_tx.clone();

        let (response_tx, response_rx) = mpsc::channel(1);

        // [ARCH-COMPLIANCE] Closure içine taşınacak değişkenlerin kopyalanması
        let trace_id_clone = trace_id.clone();
        let call_id_clone = call_id.clone();

        tokio::spawn(async move {
            if !first_msg.audio_chunk.is_empty() {
                let samples_16k: Vec<i16> = first_msg
                    .audio_chunk
                    .chunks_exact(2)
                    .map(|c| i16::from_le_bytes([c[0], c[1]]))
                    .collect();
                let samples_8k = sentiric_rtp_core::simple_resample(&samples_16k, 16000, 8000);
                let _ = egress_tx.send(samples_8k).await;
            }

            // [ARCH-COMPLIANCE] finite_state_assurance Kuralı Uygulaması
            // Eğer AI motoru 10 saniye boyunca tek bir ses chunk'ı bile göndermezse,
            // Stream bir "Zombie Task" olmamak için zorla intihar eder.
            const STREAM_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

            loop {
                match tokio::time::timeout(STREAM_IDLE_TIMEOUT, in_stream.message()).await {
                    Ok(Ok(Some(msg))) => {
                        if !msg.audio_chunk.is_empty() {
                            let samples_16k: Vec<i16> = msg
                                .audio_chunk
                                .chunks_exact(2)
                                .map(|c| i16::from_le_bytes([c[0], c[1]]))
                                .collect();
                            let samples_8k =
                                sentiric_rtp_core::simple_resample(&samples_16k, 16000, 8000);

                            if egress_tx.send(samples_8k).await.is_err() {
                                tracing::warn!(
                                    event = "EGRESS_TX_CLOSED",
                                    trace_id = %trace_id_clone,
                                    sip.call_id = %call_id_clone,
                                    "RTP Egress kanalı (session) kapandı, gRPC stream sonlandırılıyor."
                                );
                                break;
                            }
                        }
                    }
                    Ok(Ok(None)) => {
                        // EOF: İstemci (AI Pipeline) stream'i kendi isteğiyle, temiz bir şekilde kapattı.
                        tracing::debug!(
                            event = "GRPC_STREAM_EOF",
                            trace_id = %trace_id_clone,
                            sip.call_id = %call_id_clone,
                            "AI istemcisi ses akışını (Stream) başarıyla tamamladı."
                        );
                        break;
                    }
                    Ok(Err(e)) => {
                        tracing::error!(
                            event = "GRPC_STREAM_ERROR",
                            trace_id = %trace_id_clone,
                            sip.call_id = %call_id_clone,
                            error = %e,
                            "gRPC stream okunurken ağ veya protokol hatası oluştu."
                        );
                        break;
                    }
                    Err(_) => {
                        // [ARCH-COMPLIANCE] Timeout (Absolute Boundary) Devreye Girdi
                        tracing::warn!(
                            event = "GRPC_STREAM_TIMEOUT",
                            trace_id = %trace_id_clone,
                            sip.call_id = %call_id_clone,
                            "Stream {} saniye boyunca AI katmanından veri alamadı. Zombie task koruması tetiklendi ve bağlantı düşürüldü.",
                            STREAM_IDLE_TIMEOUT.as_secs()
                        );
                        break;
                    }
                }
            }

            let _ = response_tx
                .send(Ok(StreamAudioToCallResponse {
                    success: true,
                    error_message: "".to_string(),
                }))
                .await;
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(response_rx))))
    }

    #[instrument(skip(self, request), fields(call_id = %request.get_ref().call_id, trace_id))]
    async fn allocate_port(
        &self,
        request: Request<AllocatePortRequest>,
    ) -> Result<Response<AllocatePortResponse>, Status> {
        let mut trace_id = Self::extract_trace_id(&request);
        if trace_id == "unknown" {
            trace_id = format!("orphaned-{}", uuid::Uuid::new_v4());
        }
        Span::current().record("trace_id", &trace_id);

        let call_id = request.get_ref().call_id.clone();
        counter!(GRPC_REQUESTS_TOTAL, "method" => "allocate_port").increment(1);

        let port = self
            .app_state
            .port_manager
            .get_available_port()
            .await
            .ok_or(ServiceError::PortPoolExhausted)?;
        let bind_addr = format!("{}:{}", self.config.rtp_listen_ip, port);

        match UdpSocket::bind(&bind_addr).await {
            Ok(socket) => {
                gauge!(ACTIVE_SESSIONS).increment(1.0);
                let session = RtpSession::new(
                    trace_id.clone(),
                    call_id.clone(),
                    port,
                    Arc::new(socket),
                    self.app_state.clone(),
                );
                self.app_state.port_manager.add_session(port, session).await;
                //[ARCH-COMPLIANCE] sip.call_id log'a eklendi
                info!(event = "MEDIA_PORT_ALLOCATED", sip.call_id = %call_id, rtp.port = port, bind.addr = %bind_addr, "RTP Port Allocated");
                Ok(Response::new(AllocatePortResponse {
                    rtp_port: port as u32,
                }))
            }
            Err(e) => {
                warn!(event="BIND_FAIL", error=%e, addr=%bind_addr, "Socket bind başarısız");
                self.app_state.port_manager.quarantine_port(port).await;
                Err(Status::resource_exhausted("Bind failed"))
            }
        }
    }

    #[instrument(skip(self, request), fields(port = %request.get_ref().rtp_port, trace_id))]
    async fn release_port(
        &self,
        request: Request<ReleasePortRequest>,
    ) -> Result<Response<ReleasePortResponse>, Status> {
        let trace_id = Self::extract_trace_id(&request);
        Span::current().record("trace_id", &trace_id);
        let port = request.into_inner().rtp_port as u16;
        if let Some(session) = self.app_state.port_manager.get_session(port).await {
            let _ = session.send_command(RtpCommand::Shutdown).await;
            info!(event = "MEDIA_PORT_RELEASED", "RTP Port Released");
        }
        Ok(Response::new(ReleasePortResponse { success: true }))
    }

    #[instrument(skip(self, request), fields(port = %request.get_ref().server_rtp_port, trace_id))]
    async fn play_audio(
        &self,
        request: Request<PlayAudioRequest>,
    ) -> Result<Response<PlayAudioResponse>, Status> {
        let trace_id = Self::extract_trace_id(&request);
        Span::current().record("trace_id", &trace_id);
        let req = request.into_inner();
        let rtp_port = req.server_rtp_port as u16;
        let session = self
            .app_state
            .port_manager
            .get_session(rtp_port)
            .await
            .ok_or(ServiceError::SessionNotFound { port: rtp_port })?;

        let target_addr: SocketAddr =
            req.rtp_target_addr
                .parse()
                .map_err(|e| ServiceError::InvalidTargetAddress {
                    addr: req.rtp_target_addr,
                    source: e,
                })?;
        let _ = session
            .send_command(RtpCommand::SetTargetAddress {
                target: target_addr,
            })
            .await;

        if req.audio_uri.starts_with("control://") {
            let cmd = req.audio_uri.strip_prefix("control://").unwrap();
            match cmd {
                "enable_echo" => {
                    let _ = session.send_command(RtpCommand::EnableEchoTest).await;
                    return Ok(Response::new(PlayAudioResponse {
                        success: true,
                        message: "Echo On".into(),
                    }));
                }
                "disable_echo" => {
                    let _ = session.send_command(RtpCommand::DisableEchoTest).await;
                    return Ok(Response::new(PlayAudioResponse {
                        success: true,
                        message: "Echo Off".into(),
                    }));
                }
                // [YENİ]: B2BUA hedef ataması yapmak için sahte anons
                "set_target" => {
                    return Ok(Response::new(PlayAudioResponse {
                        success: true,
                        message: "Target Locked".into(),
                    }));
                }
                _ => return Err(Status::invalid_argument("Unknown control command")),
            }
        }

        let (tx, rx) = oneshot::channel();
        session
            .send_command(RtpCommand::PlayAudioUri {
                audio_uri: req.audio_uri,
                candidate_target_addr: target_addr,
                cancellation_token: tokio_util::sync::CancellationToken::new(),
                responder: Some(tx),
            })
            .await
            .map_err(|_| Status::internal("Command send fail"))?;

        match rx.await {
            Ok(Ok(_)) => Ok(Response::new(PlayAudioResponse {
                success: true,
                message: "OK".into(),
            })),
            _ => Err(Status::internal("Playback failed")),
        }
    }

    type RecordAudioStream =
        Pin<Box<dyn Stream<Item = Result<RecordAudioResponse, Status>> + Send>>;
    async fn record_audio(
        &self,
        request: Request<RecordAudioRequest>,
    ) -> Result<Response<Self::RecordAudioStream>, Status> {
        let req = request.into_inner();
        let rtp_port = req.server_rtp_port as u16;
        let session = self
            .app_state
            .port_manager
            .get_session(rtp_port)
            .await
            .ok_or(ServiceError::SessionNotFound { port: rtp_port })?;
        let (tx, rx) = mpsc::channel(64);
        session
            .send_command(RtpCommand::StartLiveAudioStream {
                stream_sender: tx,
                target_sample_rate: req.target_sample_rate,
            })
            .await
            .map_err(|_| Status::internal("Fail"))?;
        let out = ReceiverStream::new(rx).map(|r| {
            r.map(|f| RecordAudioResponse {
                audio_data: f.data.into(),
                media_type: f.media_type,
            })
        });
        Ok(Response::new(Box::pin(out)))
    }

    async fn start_recording(
        &self,
        request: Request<StartRecordingRequest>,
    ) -> Result<Response<StartRecordingResponse>, Status> {
        let req = request.into_inner();
        let session = self
            .app_state
            .port_manager
            .get_session(req.server_rtp_port as u16)
            .await
            .ok_or(Status::not_found("No session"))?;
        session
            .send_command(RtpCommand::StartPermanentRecording(RecordingSession {
                output_uri: req.output_uri,
                spec: WavSpec {
                    channels: 2,
                    sample_rate: 8000,
                    bits_per_sample: 16,
                    sample_format: SampleFormat::Int,
                },
                rx_buffer: Vec::new(),
                tx_buffer: Vec::new(),
                call_id: req.call_id,
                trace_id: req.trace_id,
                max_reached_warned: false,
            }))
            .await
            .map_err(|_| Status::internal("Command fail"))?;
        Ok(Response::new(StartRecordingResponse { success: true }))
    }

    async fn stop_recording(
        &self,
        request: Request<StopRecordingRequest>,
    ) -> Result<Response<StopRecordingResponse>, Status> {
        let session = self
            .app_state
            .port_manager
            .get_session(request.into_inner().server_rtp_port as u16)
            .await
            .ok_or(Status::not_found("No session"))?;
        let (tx, rx) = oneshot::channel();
        session
            .send_command(RtpCommand::StopPermanentRecording { responder: tx })
            .await
            .map_err(|_| Status::internal("Command fail"))?;
        match rx.await {
            Ok(Ok(_)) => Ok(Response::new(StopRecordingResponse { success: true })),
            _ => Err(Status::internal("Finalize fail")),
        }
    }
}
