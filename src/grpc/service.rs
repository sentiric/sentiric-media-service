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
};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::{wrappers::ReceiverStream, Stream, StreamExt};
use tokio_util::sync::CancellationToken;
use tonic::{Request, Response, Status};
// --- DEĞİŞİKLİK BURADA: Eksik olan `error` makrosu eklendi ---
use tracing::{debug, error, field, info, instrument, warn};
// --- DEĞİŞİKLİK SONU ---
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
    #[instrument(skip_all, fields(service = "media-service", call_id = %request.get_ref().call_id))]
    async fn allocate_port(
        &self,
        request: Request<AllocatePortRequest>,
    ) -> Result<Response<AllocatePortResponse>, Status> {
        counter!(GRPC_REQUESTS_TOTAL, "method" => "allocate_port").increment(1);
        let call_id = request.get_ref().call_id.clone();
        info!(call_id = %call_id, "Port tahsis isteği alındı.");

        const MAX_RETRIES: u8 = 5;
        for i in 0..MAX_RETRIES {
            let port_to_try = match self.app_state.port_manager.get_available_port().await {
                Some(p) => p,
                None => {
                    warn!(
                        attempt = i + 1,
                        max_attempts = MAX_RETRIES,
                        "Port havuzu tükendi."
                    );
                    if i < MAX_RETRIES - 1 {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                    return Err(ServiceError::PortPoolExhausted.into());
                }
            };

            match UdpSocket::bind(format!("{}:{}", self.config.rtp_host, port_to_try)).await {
                Ok(socket) => {
                    info!(
                        port = port_to_try,
                        "Port başarıyla bağlandı ve oturum başlatılıyor."
                    );
                    gauge!(ACTIVE_SESSIONS).increment(1.0);
                    let (tx, rx) = mpsc::channel(self.config.rtp_command_channel_buffer);
                    self.app_state
                        .port_manager
                        .add_session(port_to_try, tx)
                        .await;

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
                    self.app_state
                        .port_manager
                        .quarantine_port(port_to_try)
                        .await;
                    continue;
                }
            }
        }
        Err(ServiceError::PortPoolExhausted.into())
    }

    #[instrument(skip(self, request), fields(port = %request.get_ref().rtp_port))]
    async fn release_port(
        &self,
        request: Request<ReleasePortRequest>,
    ) -> Result<Response<ReleasePortResponse>, Status> {
        counter!(GRPC_REQUESTS_TOTAL, "method" => "release_port").increment(1);

        let port = request.into_inner().rtp_port as u16;
        if let Some(tx) = self.app_state.port_manager.get_session_sender(port).await {
            info!(port, "Oturum sonlandırma sinyali gönderiliyor.");
            if tx.send(RtpCommand::Shutdown).await.is_err() {
                warn!(
                    port,
                    "Shutdown komutu gönderilemedi (kanal zaten kapalı olabilir). Oturum muhtemelen daha önce kapatıldı."
                );
            }
        } else {
            warn!(
                port,
                "Serbest bırakılacak oturum bulunamadı veya çoktan kapatılmış."
            );
        }
        Ok(Response::new(ReleasePortResponse { success: true }))
    }

    #[instrument(skip(self, request), fields(
        port = %request.get_ref().server_rtp_port,
        audio_uri.scheme = %extract_uri_scheme(&request.get_ref().audio_uri),
        audio_uri.len = field::Empty,
    ))]
    async fn play_audio(
        &self,
        request: Request<PlayAudioRequest>,
    ) -> Result<Response<PlayAudioResponse>, Status> {
        counter!(GRPC_REQUESTS_TOTAL, "method" => "play_audio").increment(1);

        let req = request.into_inner();
        let span = tracing::Span::current();
        span.record("audio_uri.len", &req.audio_uri.len());

        if req.audio_uri.starts_with("data:") {
            let truncated_uri = &req.audio_uri[..std::cmp::min(50, req.audio_uri.len())];
            debug!(
                audio_uri.preview = %truncated_uri,
                "PlayAudio komutu (data URI) alındı."
            );
        } else {
            debug!(
                audio_uri = %req.audio_uri,
                "PlayAudio komutu (file URI) alındı."
            );
        }

        let rtp_port = req.server_rtp_port as u16;
        let tx = self
            .app_state
            .port_manager
            .get_session_sender(rtp_port)
            .await
            .ok_or_else(|| ServiceError::SessionNotFound { port: rtp_port })?;
        let target_addr = req.rtp_target_addr.parse().map_err(|e| {
            ServiceError::InvalidTargetAddress {
                addr: req.rtp_target_addr,
                source: e,
            }
        })?;

        let (responder_tx, responder_rx) = oneshot::channel();

        let cancellation_token = CancellationToken::new();
        let command = RtpCommand::PlayAudioUri {
            audio_uri: req.audio_uri,
            candidate_target_addr: target_addr,
            cancellation_token,
            responder: Some(responder_tx),
        };

        tx.send(command)
            .await
            .map_err(|_| ServiceError::CommandSendError("PlayAudioUri".to_string()))?;

        debug!(port = rtp_port, "PlayAudio komutu sıraya alındı, tamamlanması bekleniyor...");
        
        match responder_rx.await {
            Ok(Ok(_)) => {
                info!("PlayAudio işlemi başarıyla tamamlandı.");
                Ok(Response::new(PlayAudioResponse {
                    success: true,
                    message: "Playback completed successfully.".to_string(),
                }))
            }
            Ok(Err(e)) => {
                error!(error = %e, "PlayAudio işlemi hatayla tamamlandı.");
                Err(Status::internal(format!("Playback failed: {}", e)))
            }
            Err(_) => {
                error!("PlayAudio işlemi için yanıt alınamadı (responder kanalı kapandı).");
                Err(Status::internal("Playback response channel closed unexpectedly."))
            }
        }
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
        let session_tx = self
            .app_state
            .port_manager
            .get_session_sender(rtp_port)
            .await
            .ok_or_else(|| ServiceError::SessionNotFound { port: rtp_port })?;
        let (stream_tx, stream_rx) = mpsc::channel(self.config.live_audio_stream_buffer);
        let command = RtpCommand::StartLiveAudioStream {
            stream_sender: stream_tx,
            target_sample_rate: req.target_sample_rate,
        };
        session_tx
            .send(command)
            .await
            .map_err(|_| ServiceError::CommandSendError("StartLiveAudioStream".to_string()))?;
        debug!("RTP oturumuna canlı ses akışını başlatma komutu gönderildi.");
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
        output.scheme = field::Empty,
        output.bucket = field::Empty,
        output.key = field::Empty,
        call_id = %request.get_ref().call_id,
        trace_id = %request.get_ref().trace_id,
    ))]
    async fn start_recording(
        &self,
        request: Request<StartRecordingRequest>,
    ) -> Result<Response<StartRecordingResponse>, Status> {
        counter!(GRPC_REQUESTS_TOTAL, "method" => "start_recording").increment(1);
        let req_ref = request.get_ref();
        if let Ok(url) = Url::parse(&req_ref.output_uri) {
            let span = tracing::Span::current();
            span.record("output.scheme", url.scheme());
            if let Some(host) = url.host_str() {
                span.record("output.bucket", host);
            }
            span.record("output.key", url.path());
        }
        let rtp_port = req_ref.server_rtp_port as u16;
        let session_tx = self
            .app_state
            .port_manager
            .get_session_sender(rtp_port)
            .await
            .ok_or_else(|| ServiceError::SessionNotFound { port: rtp_port })?;
        let spec = WavSpec {
            channels: 1,
            sample_rate: 8000,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        
        let recording_session = RecordingSession {
            output_uri: req_ref.output_uri.clone(),
            spec,
            mixed_samples_16khz: Vec::new(),
            call_id: req_ref.call_id.clone(),
            trace_id: req_ref.trace_id.clone(),
        };

        let command = RtpCommand::StartPermanentRecording(recording_session);
        session_tx
            .send(command)
            .await
            .map_err(|_| ServiceError::CommandSendError("StartPermanentRecording".to_string()))?;
        info!("Kalıcı kayıt komutu başarıyla gönderildi.");
        Ok(Response::new(StartRecordingResponse { success: true }))
    }

    #[instrument(skip(self, request), fields(port = %request.get_ref().server_rtp_port))]
    async fn stop_recording(
        &self,
        request: Request<StopRecordingRequest>,
    ) -> Result<Response<StopRecordingResponse>, Status> {
        counter!(GRPC_REQUESTS_TOTAL, "method" => "stop_recording").increment(1);
        let req = request.into_inner();
        let rtp_port = req.server_rtp_port as u16;
        let session_tx = self
            .app_state
            .port_manager
            .get_session_sender(rtp_port)
            .await
            .ok_or_else(|| ServiceError::SessionNotFound { port: rtp_port })?;
        let (tx, rx) = oneshot::channel();
        let command = RtpCommand::StopPermanentRecording { responder: tx };
        session_tx
            .send(command)
            .await
            .map_err(|_| ServiceError::CommandSendError("StopPermanentRecording".to_string()))?;
        match rx.await {
            Ok(Ok(final_uri)) => {
                info!(uri = %final_uri, "Kayıt başarıyla tamamlandı ve kaydedildi.");
                Ok(Response::new(StopRecordingResponse { success: true }))
            }
            Ok(Err(e)) => Err(ServiceError::RecordingSaveFailed { source: e }.into()),
            Err(e) => Err(ServiceError::InternalError(anyhow::anyhow!(e)).into()),
        }
    }
}