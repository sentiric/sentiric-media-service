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

use sentiric_rtp_core::{CodecType, RtpHeader, RtpPacket, RtpEndpoint, Pacer, JitterBuffer};

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

    fn parse_rtp_packet(data: Vec<u8>) -> Option<RtpPacket> {
        if data.len() < 12 { 
            // warn!("⚠️ [RTP] Packet too short: {} bytes", data.len());
            return None; 
        }
        
        let first_byte = data[0];
        let payload_type = data[1] & 0x7F;
        let sequence_number = u16::from_be_bytes([data[2], data[3]]);
        let timestamp = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
        let ssrc = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);

        let header = RtpHeader {
            version: (first_byte >> 6) & 0x03,
            padding: (first_byte >> 5) & 0x01 != 0,
            extension: (first_byte >> 4) & 0x01 != 0,
            csrc_count: first_byte & 0x0F,
            marker: (data[1] >> 7) & 0x01 != 0,
            payload_type,
            sequence_number,
            timestamp,
            ssrc,
        };

        let payload = data[12..].to_vec();
        Some(RtpPacket { header, payload })
    }

    #[instrument(skip_all, fields(port = self.port, call_id = %self.call_id))]
    async fn run(self: Arc<Self>, socket: Arc<tokio::net::UdpSocket>, mut command_rx: mpsc::Receiver<RtpCommand>) {
        info!("🎧 RTP Session Started | JitterBuffer: ON (50pkt/60ms)");

        let live_stream_sender: Arc<Mutex<Option<mpsc::Sender<Result<AudioFrame, tonic::Status>>>>> = Arc::new(Mutex::new(None));
        let recording_session: Arc<Mutex<Option<RecordingSession>>> = Arc::new(Mutex::new(None));
        
        let endpoint = RtpEndpoint::new(None);
        let mut known_target: Option<SocketAddr> = None;
        let mut audio_processor = AudioProcessor::new(CodecType::PCMU);
        
        let mut jitter_buffer = JitterBuffer::new(50, 60);

        let mut loopback_mode_active = false;
        let mut warmer_counter: u64 = 0; 
        let mut last_rtp_received = Instant::now();

        let rtp_ssrc: u32 = rand::random();
        let mut rtp_seq: u16 = rand::random();
        let mut rtp_ts: u32 = rand::random();

        let mut packets_received = 0;
        let mut packets_sent = 0;
        let mut jitter_drops = 0;

        let (rtp_packet_tx, mut rtp_packet_rx) = mpsc::channel::<(Vec<u8>, SocketAddr)>(512);
        
        // UDP Okuyucu
        tokio::spawn({
            let socket = socket.clone();
            async move {
                let mut buf = [0u8; 2048];
                while let Ok((len, addr)) = socket.recv_from(&mut buf).await {
                    // Min RTP Header 12 byte
                    if len >= 12 { 
                        let _ = rtp_packet_tx.send((buf[..len].to_vec(), addr)).await; 
                    }
                }
            }
        });

        let mut outbound_stream_rx: Option<mpsc::Receiver<Vec<u8>>> = None;
        let mut is_streaming = false;
        let mut playback_queue: VecDeque<PlaybackJob> = VecDeque::new();
        let mut is_playing = false;
        let (finished_tx, mut finished_rx) = mpsc::channel(1);

        let mut pacer = Pacer::new(20); 
        let mut last_activity = Instant::now();
        let mut log_ticker = tokio::time::interval(Duration::from_secs(5));

        loop {
            // PACER: Her 20ms'de bir uyanır.
            pacer.wait();

            if last_activity.elapsed() > self.app_state.port_manager.config.rtp_session_inactivity_timeout {
                warn!("⚠️ Session timed out due to inactivity. Closing.");
                break;
            }

            // --- 1. JITTER BUFFER POP & PROCESS (INBOUND) ---
            if let Some(packet) = jitter_buffer.pop() {
                if let Ok(codec) = AudioCodec::from_rtp_payload_type(packet.header.payload_type) {
                    audio_processor.update_codec(codec.to_core_type());
                    
                    if let Ok(pcm) = crate::rtp::codecs::decode_rtp_to_lpcm16(&packet.payload, codec) {
                        if let Some(tx) = &*live_stream_sender.lock().await { 
                            let mut b = Vec::new(); 
                            for s in &pcm { b.extend_from_slice(&s.to_le_bytes()); }
                            let _ = tx.try_send(Ok(AudioFrame{
                                data: b.into(), 
                                media_type: "audio/L16;rate=16000".into()
                            }));
                        }
                        if let Some(rec) = &mut *recording_session.lock().await { 
                            rec.mixed_samples_16khz.extend_from_slice(&pcm); 
                        }
                    }
                }
            }

            // --- 2. OUTBOUND STREAMING (TTS / FILE) ---
            if is_streaming && !loopback_mode_active {
                if let Some(target) = endpoint.get_target().or(known_target) {
                    if let Some(rx) = &mut outbound_stream_rx {
                        while let Ok(chunk) = rx.try_recv() { audio_processor.push_data(chunk); }
                    }
                    if let Some(packets) = audio_processor.process_frame().await {
                        for p in packets { 
                            Self::send_raw_rtp(&socket, target, p, &mut rtp_seq, &mut rtp_ts, rtp_ssrc, audio_processor.get_current_codec()).await; 
                            packets_sent += 1;
                        }
                    }
                }
            }

            // [WARMER]: Keep-alive
            if loopback_mode_active && last_rtp_received.elapsed() > Duration::from_millis(150) && warmer_counter % 25 == 0 {
                if let Some(target) = endpoint.get_target().or(known_target) {
                    let hum = if warmer_counter % 50 == 0 { 0x80 } else { 0xD5 };
                    Self::send_raw_rtp(&socket, target, vec![hum; 160], &mut rtp_seq, &mut rtp_ts, rtp_ssrc, CodecType::PCMU).await;
                }
            }
            warmer_counter = warmer_counter.wrapping_add(1);

            tokio::select! {
                // İstatistik Loglama
                _ = log_ticker.tick() => {
                    // Sadece aktifse logla
                    if packets_received > 0 || packets_sent > 0 {
                        debug!("📊 Stats: Rx: {} | Tx: {} | Drops: {} | Active: {}", packets_received, packets_sent, jitter_drops, !endpoint.get_target().is_none());
                    }
                },

                // Gelen Paketler (Option döner)
                Some((data, addr)) = rtp_packet_rx.recv() => {
                    last_activity = Instant::now();
                    last_rtp_received = Instant::now();
                    packets_received += 1;

                    if endpoint.latch(addr) { info!("🔒 [LATCH] Internal media established with {}", addr); }

                    // [ECHO FIX] Jitter Buffer'ı BYPASS et!
                    // Echo testi "Sesi aldım, hemen geri yolluyorum" mantığıdır. Buffer gecikme yaratır.
                    if loopback_mode_active {
                        let mut resp = data.clone();
                        // SSRC'yi sunucunun SSRC'si ile değiştir ki "dönen" paket olduğu anlaşılsın
                        // (İlk 12 byte header'dır. SSRC 8. byte'tan başlar)
                        if resp.len() >= 12 {
                             resp[8..12].copy_from_slice(&rtp_ssrc.to_be_bytes());
                        }
                        // Geri gönder
                        let _ = socket.send_to(&resp, addr).await;
                    } else {
                        // Normal akış: Jitter Buffer'a koy
                        if let Some(packet) = Self::parse_rtp_packet(data) {
                            jitter_buffer.push(packet);
                        } else {
                            jitter_drops += 1;
                        }
                    }
                },
                
                // Komutlar
                Some(cmd) = command_rx.recv() => {
                     last_activity = Instant::now();
                     match cmd {
                        RtpCommand::EnableEchoTest => { loopback_mode_active = true; info!("🔊 Echo Mode Enabled"); },
                        RtpCommand::DisableEchoTest => loopback_mode_active = false,
                        RtpCommand::SetTargetAddress { target } => { 
                            known_target = Some(target); 
                            info!("🎯 Target address explicitly set: {}", target);
                        },
                        _ => {
                            if session_handlers::handle_command(cmd, self.port, &live_stream_sender, &recording_session, &mut outbound_stream_rx, &mut is_streaming, &mut playback_queue, &mut is_playing, &RtpSessionConfig{app_state: self.app_state.clone(), app_config: self.app_state.port_manager.config.clone(), port: self.port}, &socket, &finished_tx, &mut known_target, &endpoint, &self.call_id).await { break; }
                        }
                    }
                },

                // Playback Bitti Sinyali
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
        if let Some(rec) = recording_session.lock().await.take() { let _ = finalize_and_save_recording(rec, self.app_state.clone()).await; }
        self.app_state.port_manager.remove_session(self.port).await;
        self.app_state.port_manager.quarantine_port(self.port).await;
        gauge!(ACTIVE_SESSIONS).decrement(1.0);
        info!("🛑 RTP Session Terminated.");
    }

    async fn send_raw_rtp(socket: &tokio::net::UdpSocket, target: SocketAddr, payload: Vec<u8>, seq: &mut u16, ts: &mut u32, ssrc: u32, codec: CodecType) {
        let pt = match codec { CodecType::PCMU => 0, CodecType::PCMA => 8, CodecType::G729 => 18, CodecType::G722 => 9 };
        let header = RtpHeader::new(pt, *seq, *ts, ssrc);
        let packet = RtpPacket { header, payload };
        
        if let Err(e) = socket.send_to(&packet.to_bytes(), target).await {
            warn!("RTP send error: {}", e);
        }
        
        *seq = seq.wrapping_add(1);
        let increment = if codec.sample_rate() == 16000 { 320 } else { 160 };
        *ts = ts.wrapping_add(increment);
    }
}