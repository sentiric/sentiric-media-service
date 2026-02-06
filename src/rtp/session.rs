// sentiric-media-service/src/rtp/session.rs
use crate::metrics::ACTIVE_SESSIONS;
use crate::rtp::codecs::AudioCodec;
use crate::rtp::command::{RtpCommand, AudioFrame, RecordingSession};
use crate::rtp::handlers; // Artık handle_command ve PlaybackJob buradan doğru şekilde gelir
use crate::rtp::processing::AudioProcessor;
use crate::rtp::session_utils::finalize_and_save_recording;
use crate::state::AppState;
use crate::config::AppConfig;

use metrics::gauge;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio::time::{Duration, Instant};
use tracing::{info, instrument};

use sentiric_rtp_core::{CodecType, RtpHeader, RtpPacket, RtpEndpoint, Pacer};

#[derive(Clone)]
pub struct RtpSessionConfig {
    pub app_state: AppState,
    pub app_config: Arc<AppConfig>,
    pub port: u16,
}

pub struct RtpSession {
    pub call_id: String,
    pub port: u16,
    command_tx: mpsc::Sender<RtpCommand>,
    app_state: AppState,
}

impl RtpSession {
    pub fn new(call_id: String, port: u16, socket: Arc<tokio::net::UdpSocket>, app_state: AppState) -> Arc<Self> {
        let (command_tx, command_rx) = mpsc::channel(32);
        let session = Arc::new(Self { call_id, port, command_tx, app_state: app_state.clone() });
        tokio::spawn(Self::run(session.clone(), socket, command_rx));
        session
    }

    pub async fn send_command(&self, command: RtpCommand) -> Result<(), mpsc::error::SendError<RtpCommand>> {
        self.command_tx.send(command).await
    }

    #[instrument(skip_all, fields(port = self.port, call_id = %self.call_id))]
    async fn run(self: Arc<Self>, socket: Arc<tokio::net::UdpSocket>, mut command_rx: mpsc::Receiver<RtpCommand>) {
        info!("🎧 Session Started (System Health Optimal)");

        let live_stream_sender: Arc<Mutex<Option<mpsc::Sender<Result<AudioFrame, tonic::Status>>>>> = Arc::new(Mutex::new(None));
        let recording_session: Arc<Mutex<Option<RecordingSession>>> = Arc::new(Mutex::new(None));
        
        let endpoint = RtpEndpoint::new(None);
        let mut known_target: Option<SocketAddr> = None;
        let mut audio_processor = AudioProcessor::new(CodecType::PCMU);

        let rtp_ssrc: u32 = rand::Rng::gen(&mut rand::thread_rng());
        let mut rtp_seq: u16 = rand::Rng::gen(&mut rand::thread_rng());
        let mut rtp_ts: u32 = rand::Rng::gen(&mut rand::thread_rng());

        let mut outbound_stream_rx: Option<mpsc::Receiver<Vec<u8>>> = None;
        let mut is_streaming = false;
        let mut playback_queue: VecDeque<handlers::PlaybackJob> = VecDeque::new();
        let mut is_playing = false;
        let (finished_tx, mut finished_rx) = mpsc::channel(1);

        let (rtp_packet_tx, mut rtp_packet_rx) = mpsc::channel(256);
        let _reader = tokio::spawn({
            let socket = socket.clone();
            async move {
                let mut buf = [0u8; 2048];
                while let Ok((len, addr)) = socket.recv_from(&mut buf).await {
                    if len >= 12 && rtp_packet_tx.send((buf[..len].to_vec(), addr)).await.is_err() { break; }
                }
            }
        });

        let mut pacer = Pacer::new(Duration::from_millis(20));
        let mut last_activity = Instant::now();

        loop {
            pacer.wait();

            if last_activity.elapsed() > Duration::from_secs(60) { break; }

            if is_streaming {
                if let Some(target) = endpoint.get_target().or(known_target) {
                    if let Some(rx) = &mut outbound_stream_rx {
                        while let Ok(chunk) = rx.try_recv() { audio_processor.push_data(chunk); }
                    }
                    if let Some(packets) = audio_processor.process_frame().await {
                        for p in packets { 
                            send_rtp_packet(&socket, target, p, &mut rtp_seq, &mut rtp_ts, rtp_ssrc, audio_processor.get_current_codec()).await; 
                        }
                    }
                }
            }

            tokio::select! {
                Some(cmd) = command_rx.recv() => {
                    last_activity = Instant::now();
                    match cmd {
                        RtpCommand::SetTargetAddress { target } => { known_target = Some(target); },
                        RtpCommand::HolePunching { target_addr } => { let _ = socket.send_to(&[0u8; 160], target_addr).await; },
                        _ => if handlers::handle_command(
                            cmd, 
                            self.port, 
                            &live_stream_sender, 
                            &recording_session, 
                            &mut outbound_stream_rx, 
                            &mut is_streaming, 
                            &mut playback_queue, 
                            &mut is_playing, 
                            &RtpSessionConfig{app_state: self.app_state.clone(), app_config: self.app_state.port_manager.config.clone(), port: self.port}, 
                            &socket, 
                            &finished_tx, 
                            &mut known_target, 
                            &endpoint
                        ).await { break; }
                    }
                },
                Some((data, addr)) = rtp_packet_rx.recv() => {
                    last_activity = Instant::now();
                    if endpoint.latch(addr) { known_target = Some(addr); }
                    let pt = data[1] & 0x7F;
                    if let Ok(codec) = AudioCodec::from_rtp_payload_type(pt) {
                        audio_processor.update_codec(codec.to_core_type());
                        if let Ok(pcm) = crate::rtp::codecs::decode_rtp_to_lpcm16(&data[12..], codec) {
                            if let Some(tx) = &*live_stream_sender.lock().await { 
                                let mut bytes = Vec::new();
                                for s in &pcm { bytes.extend_from_slice(&s.to_le_bytes()); }
                                let _ = tx.try_send(Ok(AudioFrame{data: bytes.into(), media_type: "audio/L16;rate=16000".into()}));
                            }
                            if let Some(rec) = &mut *recording_session.lock().await { rec.mixed_samples_16khz.extend_from_slice(&pcm); }
                        }
                    }
                },
                Some(_) = finished_rx.recv() => {
                    is_playing = false;
                    if let Some(next) = playback_queue.pop_front() {
                        is_playing = true;
                        handlers::start_playback(next, &RtpSessionConfig{app_state: self.app_state.clone(), app_config: self.app_state.port_manager.config.clone(), port: self.port}, socket.clone(), finished_tx.clone()).await;
                    }
                }
            }
        }

        if let Some(rec) = recording_session.lock().await.take() {
            let _ = finalize_and_save_recording(rec, self.app_state.clone()).await;
        }
        self.app_state.port_manager.remove_session(self.port).await;
        self.app_state.port_manager.quarantine_port(self.port).await;
        gauge!(ACTIVE_SESSIONS).decrement(1.0);
    }
}

async fn send_rtp_packet(socket: &tokio::net::UdpSocket, target: SocketAddr, payload: Vec<u8>, seq: &mut u16, ts: &mut u32, ssrc: u32, codec: CodecType) {
    let pt = match codec { 
        CodecType::PCMU => 0, 
        CodecType::PCMA => 8, 
        CodecType::G729 => 18, 
        CodecType::G722 => 9 
    };
    let header = RtpHeader::new(pt, *seq, *ts, ssrc);
    let packet = RtpPacket { header, payload };
    let _ = socket.send_to(&packet.to_bytes(), target).await;
    *seq = seq.wrapping_add(1);
    *ts = ts.wrapping_add(if codec.sample_rate() == 16000 { 320 } else { 160 });
}