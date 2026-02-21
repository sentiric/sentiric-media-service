use crate::metrics::{ACTIVE_SESSIONS};
use crate::rtp::codecs::AudioCodec;
use crate::rtp::command::{RtpCommand, AudioFrame, RecordingSession};
use crate::rtp::session_handlers; 
use crate::rtp::processing::AudioProcessor;
use crate::rtp::session_utils::finalize_and_save_recording;
use crate::state::AppState;
use crate::config::AppConfig;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio::time::{Duration, Instant};
use tracing::{info, instrument, warn};
use metrics::gauge;
use sentiric_rtp_core::{RtpHeader, RtpPacket, RtpEndpoint, Pacer, JitterBuffer, AudioProfile};

#[derive(Clone)]
pub struct RtpSessionConfig {
    pub app_state: AppState,
    pub app_config: Arc<AppConfig>,
    pub port: u16,
}

pub struct RtpSession {
    pub call_id: String,
    pub trace_id: String, // EKLENDİ
    pub port: u16,
    command_tx: mpsc::Sender<RtpCommand>,
    app_state: AppState,
}

impl RtpSession {
    // İMZA DEĞİŞTİ: trace_id parametresi eklendi
    pub fn new(trace_id: String, call_id: String, port: u16, socket: Arc<tokio::net::UdpSocket>, app_state: AppState) -> Arc<Self> {
        let (command_tx, command_rx) = mpsc::channel(128);
        let session = Arc::new(Self { 
            call_id, 
            trace_id, 
            port, 
            command_tx, 
            app_state: app_state.clone() 
        });
        tokio::spawn(Self::run(session.clone(), socket, command_rx));
        session
    }

    pub async fn send_command(&self, command: RtpCommand) -> Result<(), mpsc::error::SendError<RtpCommand>> {
        self.command_tx.send(command).await
    }

    fn parse_rtp_packet(data: Vec<u8>) -> Option<RtpPacket> {
        if data.len() < 12 { return None; }
        let header = RtpHeader {
            version: (data[0] >> 6) & 0x03,
            padding: (data[0] >> 5) & 0x01 != 0,
            extension: (data[0] >> 4) & 0x01 != 0,
            csrc_count: data[0] & 0x0F,
            marker: (data[1] >> 7) & 0x01 != 0,
            payload_type: data[1] & 0x7F,
            sequence_number: u16::from_be_bytes([data[2], data[3]]),
            timestamp: u32::from_be_bytes([data[4], data[5], data[6], data[7]]),
            ssrc: u32::from_be_bytes([data[8], data[9], data[10], data[11]]),
        };
        let payload = data[12..].to_vec();
        Some(RtpPacket { header, payload })
    }

    #[instrument(skip_all, fields(port = self.port, call_id = %self.call_id, trace_id = %self.trace_id))]
    async fn run(self: Arc<Self>, socket: Arc<tokio::net::UdpSocket>, mut command_rx: mpsc::Receiver<RtpCommand>) {
        info!(
            event = "RTP_SESSION_START",
            trace_id = %self.trace_id,
            call_id = %self.call_id,
            "🎧 RTP Session Started"
        );

        let live_stream_sender: Arc<Mutex<Option<mpsc::Sender<Result<AudioFrame, tonic::Status>>>>> = Arc::new(Mutex::new(None));
        let recording_session: Arc<Mutex<Option<RecordingSession>>> = Arc::new(Mutex::new(None));
        let endpoint = RtpEndpoint::new(None);
        let _audio_processor = AudioProcessor::new(AudioProfile::default().preferred_audio_codec());
        let mut jitter_buffer = JitterBuffer::new(50, 60);

        let mut last_seq: Option<u16> = None;
        let mut packet_loss_count = 0u64;
        let mut total_packets_rx = 0u64;
        let mut jitter_acc = 0.0f64;
        let mut last_arrival = Instant::now();

        let (rtp_packet_tx, mut rtp_packet_rx) = mpsc::channel::<(Vec<u8>, SocketAddr)>(1024);
        tokio::spawn({
            let socket = socket.clone();
            async move {
                let mut buf = [0u8; 2048];
                while let Ok((len, addr)) = socket.recv_from(&mut buf).await {
                    let _ = rtp_packet_tx.send((buf[..len].to_vec(), addr)).await; 
                }
            }
        });

        let mut outbound_stream_rx: Option<mpsc::Receiver<Vec<u8>>> = None;
        let mut is_streaming = false;
        let mut playback_queue: std::collections::VecDeque<session_handlers::PlaybackJob> = std::collections::VecDeque::new();
        let mut is_playing = false;
        let (finished_tx, mut finished_rx) = mpsc::channel(1);
        let mut pacer = Pacer::new(20); 
        let mut stats_ticker = tokio::time::interval(Duration::from_secs(5));
        let mut last_activity = Instant::now();

        let session_config = RtpSessionConfig {
            app_state: self.app_state.clone(),
            app_config: self.app_state.port_manager.config.clone(),
            port: self.port,
        };

        loop {
            pacer.wait();

            if last_activity.elapsed() > self.app_state.port_manager.config.rtp_session_inactivity_timeout {
                warn!(
                    event = "RTP_TIMEOUT",
                    trace_id = %self.trace_id,
                    "⚠️ Session inactivity timeout. Closing."
                );
                break;
            }

            if let Some(packet) = jitter_buffer.pop() {
                if let Ok(codec) = AudioCodec::from_rtp_payload_type(packet.header.payload_type) {
                    if let Ok(pcm) = crate::rtp::codecs::decode_rtp_to_lpcm16(&packet.payload, codec) {
                        if let Some(tx) = &*live_stream_sender.lock().await { 
                            let mut b = Vec::new(); 
                            for s in &pcm { b.extend_from_slice(&s.to_le_bytes()); }
                            let _ = tx.try_send(Ok(AudioFrame{ data: b.into(), media_type: "audio/L16;rate=16000".into() }));
                        }
                        if let Some(rec) = &mut *recording_session.lock().await { 
                            rec.mixed_samples_16khz.extend_from_slice(&pcm); 
                        }
                    }
                }
            }

            tokio::select! {
                _ = stats_ticker.tick() => {
                    let loss_rate = if total_packets_rx > 0 { 
                        (packet_loss_count as f64 / (total_packets_rx + packet_loss_count) as f64) * 100.0 
                    } else { 0.0 };
                    let avg_jitter = if total_packets_rx > 0 { jitter_acc / total_packets_rx as f64 } else { 0.0 };
                    
                    // [SUTS v4.0]: Zenginleştirilmiş QoS Raporu
                    info!(
                        event = "RTP_QOS",
                        trace_id = %self.trace_id,
                        call_id = %self.call_id,
                        packet_loss_percent = loss_rate,
                        jitter_ms = avg_jitter,
                        packets_received = total_packets_rx,
                        "RTP Quality Report"
                    );
                },

                Some((data, addr)) = rtp_packet_rx.recv() => {
                    last_activity = Instant::now();
                    total_packets_rx += 1;
                    if endpoint.latch(addr) { 
                        info!(
                            event = "RTP_LATCH",
                            trace_id = %self.trace_id,
                            peer.ip = %addr.ip(),
                            peer.port = addr.port(),
                            "🔒 Media Latched"
                        ); 
                    }

                    if let Some(packet) = Self::parse_rtp_packet(data) {
                        let now = Instant::now();
                        let delta = now.duration_since(last_arrival).as_millis() as f64;
                        jitter_acc += (delta - 20.0).abs();
                        last_arrival = now;

                        if let Some(prev) = last_seq {
                            let expected = prev.wrapping_add(1);
                            if packet.header.sequence_number != expected {
                                packet_loss_count += packet.header.sequence_number.wrapping_sub(expected) as u64;
                            }
                        }
                        last_seq = Some(packet.header.sequence_number);
                        jitter_buffer.push(packet);
                    }
                },
                
                Some(cmd) = command_rx.recv() => {
                     last_activity = Instant::now();
                     if matches!(cmd, RtpCommand::Shutdown) { break; }
                     
                     if session_handlers::handle_command(
                         cmd, self.port, &live_stream_sender, &recording_session, 
                         &mut outbound_stream_rx, &mut is_streaming, &mut playback_queue, 
                         &mut is_playing, 
                         &session_config, 
                         &socket, &finished_tx, &mut None, &endpoint, &self.call_id
                     ).await { break; }
                },

                Some(_) = finished_rx.recv() => {
                     is_playing = false;
                     if let Some(next) = playback_queue.pop_front() {
                         is_playing = true;
                         session_handlers::start_playback(next, &session_config, socket.clone(), finished_tx.clone(), &self.call_id).await;
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
        
        info!(
            event = "RTP_SESSION_END",
            trace_id = %self.trace_id,
            "🛑 RTP Session Terminated."
        );
    }
}