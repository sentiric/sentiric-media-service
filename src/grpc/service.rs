use crate::config::AppConfig;
use crate::grpc::error::ServiceError;
use crate::metrics::{ACTIVE_SESSIONS, GRPC_REQUESTS_TOTAL};
use crate::rtp::command::{RecordingSession, RtpCommand};
use crate::rtp::session::{rtp_session_handler, RtpSessionConfig};
use crate::state::AppState;
use crate::{
    AllocatePortRequest, AllocatePortResponse, MediaService, PlayAudioRequest, PlayAudioResponse,
    RecordAudioRequest, RecordAudioResponse, ReleasePortRequest, ReleasePortResponse,
    StartRecordingRequest, StartRecordingResponse, StopRecordingRequest, StopRecordingResponse,
};
use hound::{SampleFormat, WavSpec};
use metrics::{counter, gauge};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::{wrappers::ReceiverStream, Stream, StreamExt};
use tokio_util::sync::CancellationToken;
use tonic::{Request, Response, Status};
use tracing::{info, instrument, warn};

pub struct MyMediaService {
    app_state: AppState,
    config: Arc<AppConfig>,
}

impl MyMediaService {
    pub fn new(config: Arc<AppConfig>, app_state: AppState) -> Self {
        Self { app_state, config }
    }
}

fn extract_uri_scheme(uri: &str) -> &str {
    if let Some(scheme_end) = uri.find(':') { &uri[..scheme_end] } else { "unknown" }
}

fn get_trace_id_from_metadata<T>(request: &Request<T>) -> String {
    request
        .metadata()
        .get("x-trace-id")
        .and_then(|value| value.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("trace-{}", uuid::Uuid::new_v4()))
}

#[tonic::async_trait]
impl MediaService for MyMediaService {
    #[instrument(skip(self, request), fields(service = "media-service", call_id = %request.get_ref().call_id, trace_id = %get_trace_id_from_metadata(&request)))]
    async fn allocate_port(&self, request: Request<AllocatePortRequest>) -> Result<Response<AllocatePortResponse>, Status> {
        counter!(GRPC_REQUESTS_TOTAL, "method" => "allocate_port").increment(1);
        let call_id = request.get_ref().call_id.clone();
        info!(call_id = %call_id, "Port tahsis isteği alındı.");
        const MAX_RETRIES: u8 = 5;
        for i in 0..MAX_RETRIES {
            let port_to_try = match self.app_state.port_manager.get_available_port().await {
                Some(p) => p,
                None => {
                    warn!(attempt = i + 1, max_attempts = MAX_RETRIES, "Port havuzu tükendi.");
                    if i < MAX_RETRIES - 1 { tokio::time::sleep(Duration::from_millis(100)).await; continue; }
                    return Err(ServiceError::PortPoolExhausted.into());
                }
            };
            match UdpSocket::bind(format!("{}:{}", self.config.rtp_host, port_to_try)).await {
                Ok(socket) => {
                    info!(port = port_to_try, "Port başarıyla bağlandı ve oturum başlatılıyor.");
                    gauge!(ACTIVE_SESSIONS).increment(1.0);
                    let (tx, rx) = mpsc::channel(self.config.rtp_command_channel_buffer);
                    self.app_state.port_manager.add_session(port_to_try, tx).await;

                    let session_config = RtpSessionConfig {
                        app_state: self.app_state.clone(),
                        app_config: self.config.clone(),
                        port: port_to_try,
                    };

                    tokio::spawn(rtp_session_handler(Arc::new(socket), rx, session_config));
                    return Ok(Response::new(AllocatePortResponse { rtp_port: port_to_try as u32 }));
                },
                Err(e) => {
                    warn!(port = port_to_try, error = %e, "Porta bağlanılamadı, karantinaya alınıp başka port denenecek.");
                    self.app_state.port_manager.quarantine_port(port_to_try).await;
                    continue;
                }
            }
        }
        Err(ServiceError::PortPoolExhausted.into())
    }

    #[instrument(skip(self, request), fields(port = %request.get_ref().rtp_port))]
    async fn release_port(&self, request: Request<ReleasePortRequest>) -> Result<Response<ReleasePortResponse>, Status> {
        counter!(GRPC_REQUESTS_TOTAL, "method" => "release_port").increment(1);

        let port = request.into_inner().rtp_port as u16;
        if let Some(tx) = self.app_state.port_manager.get_session_sender(port).await {
            info!(port, "Oturum sonlandırma sinyali gönderiliyor.");
            if tx.send(RtpCommand::Shutdown).await.is_err() {
                warn!(port, "Shutdown komutu gönderilemedi (kanal zaten kapalı olabilir).");
                gauge!(ACTIVE_SESSIONS).decrement(1.0);
            }
        } else {
            warn!(port, "Serbest bırakılacak oturum bulunamadı veya çoktan kapatılmış.");
        }
        Ok(Response::new(ReleasePortResponse { success: true }))
    }

    #[instrument(skip(self, request), fields(port = request.get_ref().server_rtp_port, uri_scheme = extract_uri_scheme(&request.get_ref().audio_uri)))]
    async fn play_audio(&self, request: Request<PlayAudioRequest>) -> Result<Response<PlayAudioResponse>, Status> {
        counter!(GRPC_REQUESTS_TOTAL, "method" => "play_audio").increment(1);
        let req = request.into_inner();
        let rtp_port = req.server_rtp_port as u16;

        let tx = self.app_state.port_manager.get_session_sender(rtp_port).await
            .ok_or_else(|| ServiceError::SessionNotFound { port: rtp_port })?;

        let target_addr = req.rtp_target_addr.parse().map_err(|e| {
            ServiceError::InvalidTargetAddress { addr: req.rtp_target_addr, source: e }
        })?;

        if tx.send(RtpCommand::StopAudio).await.is_err() {
            return Err(ServiceError::CommandSendError("StopAudio".to_string()).into());
        }

        let cancellation_token = CancellationToken::new();
        let command = RtpCommand::PlayAudioUri {
            audio_uri: req.audio_uri,
            candidate_target_addr: target_addr,
            cancellation_token,
        };

        if tx.send(command).await.is_err() {
            return Err(ServiceError::CommandSendError("PlayAudioUri".to_string()).into());
        }

        info!(port = rtp_port, "PlayAudio komutu başarıyla sıraya alındı.");

        Ok(Response::new(PlayAudioResponse {
            success: true,
            message: "Playback command successfully queued.".to_string(),
        }))
    }

    type RecordAudioStream = Pin<Box<dyn Stream<Item = Result<RecordAudioResponse, Status>> + Send>>;
    #[instrument(skip(self, request), fields(port = %request.get_ref().server_rtp_port))]
    async fn record_audio(
        &self,
        request: Request<RecordAudioRequest>,
    ) -> Result<Response<Self::RecordAudioStream>, Status> {
        counter!(GRPC_REQUESTS_TOTAL, "method" => "record_audio").increment(1);
        let req = request.into_inner();
        let rtp_port = req.server_rtp_port as u16;

        info!("Canlı ses kaydı stream isteği alındı.");

        let session_tx = self.app_state.port_manager.get_session_sender(rtp_port)
            .await
            .ok_or_else(|| ServiceError::SessionNotFound { port: rtp_port })?;

        let (stream_tx, stream_rx) = mpsc::channel(self.config.live_audio_stream_buffer);

        let command = RtpCommand::StartLiveAudioStream {
            stream_sender: stream_tx,
            target_sample_rate: req.target_sample_rate,
        };

        if session_tx.send(command).await.is_err() {
            return Err(ServiceError::CommandSendError("StartLiveAudioStream".to_string()).into());
        }

        info!("RTP oturumuna canlı ses akışını başlatma komutu gönderildi.");

        let output_stream = ReceiverStream::new(stream_rx).map(|res| {
            res.map(|frame| RecordAudioResponse {
                audio_data: frame.data.into(),
                media_type: frame.media_type,
            })
        });

        Ok(Response::new(Box::pin(output_stream)))
    }

    #[instrument(skip(self, request), fields(port = %request.get_ref().server_rtp_port, uri = %request.get_ref().output_uri))]
    async fn start_recording(&self, request: Request<StartRecordingRequest>) -> Result<Response<StartRecordingResponse>, Status> {
        counter!(GRPC_REQUESTS_TOTAL, "method" => "start_recording").increment(1);
        let req_ref = request.get_ref();
        let rtp_port = req_ref.server_rtp_port as u16;

        let session_tx = self.app_state.port_manager.get_session_sender(rtp_port).await
            .ok_or_else(|| ServiceError::SessionNotFound { port: rtp_port })?;

        let call_id = req_ref.call_id.clone();
        let trace_id = req_ref.trace_id.clone();

        let spec = WavSpec {
            channels: 1,
            sample_rate: req_ref.sample_rate.unwrap_or(16000),
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };

        // DEĞİŞİKLİK: Yeni RecordingSession yapısına uygun başlatma.
        let recording_session = RecordingSession {
            output_uri: req_ref.output_uri.clone(),
            spec,
            inbound_samples: Vec::new(),
            outbound_samples: Vec::new(),
            call_id,
            trace_id,
        };

        let command = RtpCommand::StartPermanentRecording(recording_session);
        session_tx.send(command).await
            .map_err(|_| ServiceError::CommandSendError("StartPermanentRecording".to_string()))?;
        info!("Kalıcı kayıt komutu başarıyla gönderildi.");
        Ok(Response::new(StartRecordingResponse { success: true }))
    }

    #[instrument(skip(self, request), fields(port = %request.get_ref().server_rtp_port))]
    async fn stop_recording(&self, request: Request<StopRecordingRequest>) -> Result<Response<StopRecordingResponse>, Status> {
        counter!(GRPC_REQUESTS_TOTAL, "method" => "stop_recording").increment(1);
        let req = request.into_inner();
        let rtp_port = req.server_rtp_port as u16;
        let session_tx = self.app_state.port_manager.get_session_sender(rtp_port).await
            .ok_or_else(|| ServiceError::SessionNotFound { port: rtp_port })?;

        let (tx, rx) = oneshot::channel();

        let command = RtpCommand::StopPermanentRecording { responder: tx };
        if session_tx.send(command).await.is_err() {
            return Err(ServiceError::CommandSendError("StopPermanentRecording".to_string()).into());
        }

        match rx.await {
            Ok(Ok(final_uri)) => {
                info!(uri = %final_uri, "Kayıt başarıyla tamamlandı ve kaydedildi.");
                Ok(Response::new(StopRecordingResponse { success: true }))
            }
            Ok(Err(e)) => {
                Err(ServiceError::RecordingSaveFailed { source: e }.into())
            }
            Err(e) => {
                Err(ServiceError::InternalError(anyhow::anyhow!(e)).into())
            }
        }
    }
}