// sentiric-media-service/src/rtp/session.rs
use crate::metrics::ACTIVE_SESSIONS;
use crate::rtp::codecs::AudioCodec;
use crate::rtp::command::{RtpCommand, AudioFrame, RecordingSession};
use crate::rtp::session_handlers::{self, PlaybackJob}; 
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
use tracing::{info, instrument, warn, debug};

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
        let (command_tx, command_rx) = mpsc::channel(128);
        let session = Arc::new(Self { call_id, port, command_tx, app_state: app_state.clone() });
        tokio::spawn(Self::run(session.clone(), socket, command_rx));
        session
    }

    pub async fn send_command(&self, command: RtpCommand) -> Result<(), mpsc::error::SendError<RtpCommand>> {
        self.command_tx.send(command).await
    }

    #[instrument(skip_all, fields(port = self.port, call_id = %self.call_id))]
    async fn run(self: Arc<Self>, socket: Arc<tokio::net::UdpSocket>, mut command_rx: mpsc::Receiver<RtpCommand>) {
        info!("🎧 Iron Core RTP v1.3.1 Active");

        let live_stream_sender: Arc<Mutex<Option<mpsc::Sender<Result<AudioFrame, tonic::Status>>>>> = Arc::new(Mutex::new(None));
        let recording_session: Arc<Mutex<Option<RecordingSession>>> = Arc::new(Mutex::new(None));
        
        let endpoint = RtpEndpoint::new(None);
        let mut known_target: Option<SocketAddr> = None;
        let mut audio_processor = AudioProcessor::new(CodecType::PCMU);
        
        let mut loopback_mode_active = false;
        let mut warmer_tick: u64 = 0; 
        let mut last_rtp_received = Instant::now();

        let rtp_ssrc: u32 = rand::random();
        let mut rtp_seq: u16 = rand::random();
        let mut rtp_ts: u32 = rand::random();

        let mut outbound_stream_rx: Option<mpsc::Receiver<Vec<u8>>> = None;
        let mut is_streaming = false;
        let mut playback_queue: VecDeque<PlaybackJob> = VecDeque::new();
        let mut is_playing = false;
        let (finished_tx, mut finished_rx) = mpsc::channel(1);

        let (rtp_packet_tx, mut rtp_packet_rx) = mpsc::channel::<(Vec<u8>, SocketAddr)>(512);
        let _reader_handle = tokio::spawn({
            let socket = socket.clone();
            async move {
                let mut buf = [0u8; 2048];
                while let Ok((len, addr)) = socket.recv_from(&mut buf).await {
                    if len >= 12 { let _ = rtp_packet_tx.send((buf[..len].to_vec(), addr)).await; }
                }
            }
        });

        let mut pacer = Pacer::new(20); 
        let mut last_activity = Instant::now();

        loop {
            pacer.wait();

            if last_activity.elapsed() > self.app_state.port_manager.config.rtp_session_inactivity_timeout {
                break;
            }

            // [v2.6 MİMARİ]: SMART WARMER / SIGNATURE HUM
            // Sadece Echo modu aktifse VE dışarıdan ses gelmiyorsa (sessizlik anı) tüneli 'ısıt'.
            if loopback_mode_active && last_rtp_received.elapsed() > Duration::from_millis(150) && warmer_tick % 25 == 0 {
                if let Some(target) = endpoint.get_target().or(known_target) {
                    // Sentiric Signature: Düşük frekanslı diagnostic tone (440Hz)
                    let tone = generate_diag_sample(warmer_tick);
                    Self::send_raw_rtp(&socket, target, vec![tone; 160], &mut rtp_seq, &mut rtp_ts, rtp_ssrc, CodecType::PCMU).await;
                }
            }
            warmer_tick = warmer_tick.wrapping_add(1);

            // Normal Outbound (AI Response)
            if is_streaming && !loopback_mode_active {
                if let Some(target) = endpoint.get_target().or(known_target) {
                    if let Some(rx) = &mut outbound_stream_rx {
                        while let Ok(chunk) = rx.try_recv() { audio_processor.push_data(chunk); }
                    }
                    if let Some(packets) = audio_processor.process_frame().await {
                        for p in packets { 
                            Self::send_raw_rtp(&socket, target, p, &mut rtp_seq, &mut rtp_ts, rtp_ssrc, audio_processor.get_current_codec()).await; 
                        }
                    }
                }
            }

            tokio::select! {
                Some(cmd) = command_rx.recv() => {
                    last_activity = Instant::now();
                    match cmd {
                        RtpCommand::EnableEchoTest => { loopback_mode_active = true; info!("🔊 Sentiric Echo Plus ENABLED."); },
                        RtpCommand::DisableEchoTest => loopback_mode_active = false,
                        RtpCommand::SetTargetAddress { target } => { known_target = Some(target); },
                        _ => {
                            if session_handlers::handle_command(cmd, self.port, &live_stream_sender, &recording_session, &mut outbound_stream_rx, &mut is_streaming, &mut playback_queue, &mut is_playing, &RtpSessionConfig{app_state: self.app_state.clone(), app_config: self.app_state.port_manager.config.clone(), port: self.port}, &socket, &finished_tx, &mut known_target, &endpoint, &self.call_id).await { break; }
                        }
                    }
                },
                Some((data, addr)) = rtp_packet_rx.recv() => {
                    last_activity = Instant::now();
                    last_rtp_received = Instant::now(); // [KRİTİK]: Warmer'ı sustur.

                    if endpoint.latch(addr) { info!("🔒 [LATCH] Path Established: {}", addr); }

                    if loopback_mode_active {
                        // [v2.6 FIX]: BIT-PERFECT REFLEX
                        // Gelen paketi HİÇ değiştirmeden geri yolla (SSRC manipülasyonu kaldırıldı).
                        // Bu, Baresip'in jitter buffer'ının bozulmasını engeller.
                        let _ = socket.send_to(&data, addr).await;
                        continue; 
                    }

                    // AI Processing (STT)
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
                        session_handlers::start_playback(next, &RtpSessionConfig{app_state: self.app_state.clone(), app_config: self.app_state.port_manager.config.clone(), port: self.port}, socket.clone(), finished_tx.clone(), &self.call_id).await;
                    }
                }
            }
        }

        endpoint.reset(); 
        if let Some(rec) = recording_session.lock().await.take() {
            let _ = finalize_and_save_recording(rec, self.app_state.clone()).await;
        }
        self.app_state.port_manager.remove_session(self.port).await;
        self.app_state.port_manager.quarantine_port(self.port).await;
        gauge!(ACTIVE_SESSIONS).decrement(1.0);
    }

    async fn send_raw_rtp(socket: &tokio::net::UdpSocket, target: SocketAddr, payload: Vec<u8>, seq: &mut u16, ts: &mut u32, ssrc: u32, codec: CodecType) {
        let pt = match codec { CodecType::PCMU => 0, CodecType::PCMA => 8, CodecType::G729 => 18, CodecType::G722 => 9 };
        let header = RtpHeader::new(pt, *seq, *ts, ssrc);
        let packet = RtpPacket { header, payload };
        let _ = socket.send_to(&packet.to_bytes(), target).await;
        *seq = seq.wrapping_add(1);
        *ts = ts.wrapping_add(160);
    }
}

fn generate_diag_sample(tick: u64) -> u8 {
    let freq = 440.0;
    let t = tick as f32 / (8000.0 / freq);
    if (t * 2.0 * std::f32::consts::PI).sin() > 0.0 { 0x80 } else { 0xD5 }
}