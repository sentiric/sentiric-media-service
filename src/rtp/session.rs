// src/rtp/session.rs
use crate::rtp::codecs::AudioCodec;
use crate::rtp::command::{RtpCommand, AudioFrame, RecordingSession};
use crate::rtp::session_handlers; 
use crate::rtp::processing::AudioProcessor;
use crate::state::AppState;
use crate::config::AppConfig;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio::time::{Duration, Instant};
use tracing::{info, error, instrument, warn, debug}; 

use crate::metrics::{ACTIVE_SESSIONS, RECORDING_BUFFER_BYTES};
use metrics::gauge;

use sentiric_rtp_core::{RtpHeader, RtpPacket, RtpEndpoint, JitterBuffer, AudioProfile, CodecFactory, Decoder};

#[derive(Clone)]
pub struct RtpSessionConfig {
    pub app_state: AppState,
    pub app_config: Arc<AppConfig>,
    pub port: u16,
}

pub struct RtpSession {
    pub call_id: String,
    pub trace_id: String,
    pub port: u16,
    command_tx: mpsc::Sender<RtpCommand>,
    app_state: AppState,
}

impl RtpSession {
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
        let gain_multiplier = self.app_state.port_manager.config.audio_recording_gain;
        
        info!(
            event = "RTP_SESSION_START",
            resource.service.name = "media-service",
            sip.call_id = %self.call_id,
            audio.gain = gain_multiplier,
            "🎧 RTP Oturumu Başlatıldı (Stateful Decoder & Gain Ready)"
        );

        let live_stream_sender: Arc<Mutex<Option<mpsc::Sender<Result<AudioFrame, tonic::Status>>>>> = Arc::new(Mutex::new(None));
        let recording_session: Arc<Mutex<Option<RecordingSession>>> = Arc::new(Mutex::new(None));
        let endpoint = RtpEndpoint::new(None);
        let _audio_processor = AudioProcessor::new(AudioProfile::default().preferred_audio_codec());
        
        let mut jitter_buffer = JitterBuffer::new(100, 60);

        let mut last_seq: Option<u16> = None;
        let mut packet_loss_count = 0u64;
        let mut total_packets_rx = 0u64;
        let mut jitter_acc = 0.0f64;
        let mut last_arrival = Instant::now();
        
        let mut echo_mode = false;
        let mut echo_tx_count = 0u64;
        let mut known_target: Option<SocketAddr> = None;

        let server_ssrc: u32 = rand::random();
        let mut echo_seq: u16 = rand::random();
        let mut echo_ts: u32 = rand::random();

        let (rtp_packet_tx, mut rtp_packet_rx) = mpsc::channel::<(Vec<u8>, SocketAddr)>(2048); 
        
        tokio::spawn({
            let socket = socket.clone();
            async move {
                let mut buf = [0u8; 2048];
                while let Ok((len, addr)) = socket.recv_from(&mut buf).await {
                    let _ = rtp_packet_tx.try_send((buf[..len].to_vec(), addr)); 
                }
            }
        });

        let mut outbound_stream_rx: Option<mpsc::Receiver<Vec<u8>>> = None;
        let mut is_streaming = false;
        let mut playback_queue: std::collections::VecDeque<session_handlers::PlaybackJob> = std::collections::VecDeque::new();
        let mut is_playing = false;
        let (finished_tx, mut finished_rx) = mpsc::channel(1);
        
        let mut stats_ticker = tokio::time::interval(Duration::from_secs(5));
        let mut latch_ticker = tokio::time::interval(Duration::from_millis(100));
        let mut last_activity = Instant::now();

        let session_config = RtpSessionConfig {
            app_state: self.app_state.clone(),
            app_config: self.app_state.port_manager.config.clone(),
            port: self.port,
        };

        let mut active_decoder: Option<Box<dyn Decoder>> = None;
        let mut active_payload_type: Option<u8> = None;

        loop {
            let timeout = session_config.app_config.rtp_session_inactivity_timeout;

            if last_activity.elapsed() > timeout {
                warn!(
                    event = "RTP_TIMEOUT",
                    sip.call_id = %self.call_id,
                    timeout_secs = timeout.as_secs(),
                    "⚠️ Oturum hareketsizlik zaman aşımı. Kapatılıyor."
                );
                break;
            }

            for _ in 0..10 {
                if let Some(packet) = jitter_buffer.pop() {
                    
                    if echo_mode {
                        if let Some(target) = known_target.or_else(|| endpoint.get_target()) {
                            let ts_increment = match packet.header.payload_type {
                                18 => (packet.payload.len() as u32) * 8, 
                                0 | 8 => packet.payload.len() as u32,    
                                _ => 160, 
                            };
                            let header = RtpHeader::new(packet.header.payload_type, echo_seq, echo_ts, server_ssrc);
                            let out_packet = RtpPacket { header, payload: packet.payload.clone() };
                            let _ = socket.send_to(&out_packet.to_bytes(), target).await;
                            
                            echo_seq = echo_seq.wrapping_add(1);
                            echo_ts = echo_ts.wrapping_add(ts_increment); 
                            echo_tx_count += 1;
                        }
                    }

                    if let Ok(codec) = AudioCodec::from_rtp_payload_type(packet.header.payload_type) {
                        
                        if active_payload_type != Some(packet.header.payload_type) {
                            active_decoder = Some(CodecFactory::create_decoder(codec.to_core_type()));
                            active_payload_type = Some(packet.header.payload_type);
                            
                            debug!(
                                event = "DECODER_READY", 
                                codec = ?codec, 
                                audio.gain = gain_multiplier,
                                "✅ Akıllı RTP Decoder ve Digital Gain motoru devrede."
                            );
                        }

                        if let Some(ref mut decoder) = active_decoder {
                            if codec != AudioCodec::TelephoneEvent {
                                let raw_pcm_8k = decoder.decode(&packet.payload);

                                let pcm_8k: Vec<i16> = if (gain_multiplier - 1.0).abs() > f32::EPSILON {
                                    raw_pcm_8k.into_iter().map(|sample| {
                                        let boosted = (sample as f32 * gain_multiplier) as i32;
                                        boosted.clamp(i16::MIN as i32, i16::MAX as i32) as i16
                                    }).collect()
                                } else {
                                    raw_pcm_8k 
                                };

                                if live_stream_sender.lock().await.is_some() {
                                    if let Some(tx) = &*live_stream_sender.lock().await { 
                                        let pcm_16k = sentiric_rtp_core::simple_resample(&pcm_8k, 8000, 16000);
                                        let mut b = Vec::with_capacity(pcm_16k.len() * 2); 
                                        for s in &pcm_16k { b.extend_from_slice(&s.to_le_bytes()); }
                                        let _ = tx.try_send(Ok(AudioFrame{ data: b.into(), media_type: "audio/L16;rate=16000".into() }));
                                    }
                                }

                                if let Some(rec) = &mut *recording_session.lock().await { 
                                    const MAX_SAMPLES: usize = 57_600_000; 
                                    
                                    if rec.audio_buffer.len() + pcm_8k.len() <= MAX_SAMPLES {
                                        rec.audio_buffer.extend_from_slice(&pcm_8k); 
                                        gauge!(RECORDING_BUFFER_BYTES).increment((pcm_8k.len() * 2) as f64);
                                    } else if !rec.max_reached_warned {
                                        warn!(
                                            event = "MAX_CALL_DURATION_REACHED",
                                            sip.call_id = %self.call_id,
                                            max_samples = MAX_SAMPLES,
                                            "⚠️ OOM Koruması: Çağrı maksimum kayıt süresini aştı. Kayıt donduruldu."
                                        );
                                        rec.max_reached_warned = true;
                                    }
                                }
                            }
                        }
                    }
                } else {
                    break; 
                }
            }

            tokio::select! {
                _ = stats_ticker.tick() => {
                    let loss_rate = if total_packets_rx > 0 { (packet_loss_count as f64 / (total_packets_rx + packet_loss_count) as f64) * 100.0 } else { 0.0 };
                    let avg_jitter = if total_packets_rx > 0 { jitter_acc / total_packets_rx as f64 } else { 0.0 };
                    
                    if total_packets_rx > 0 {
                        info!(
                            event = "RTP_QOS_REPORT",
                            sip.call_id = %self.call_id,
                            packet_loss_pct = loss_rate,
                            jitter_ms = avg_jitter,
                            rx_count = total_packets_rx,
                            echo_tx_count = echo_tx_count,
                            "RTP Kalite Raporu"
                        );
                    }
                },

                _ = latch_ticker.tick() => {
                    if endpoint.get_target().is_none() {
                        if let Some(target) = known_target {
                            let dummy_rtp = vec![
                                0x80, 0x00, 0x00, 0x01, 
                                0x00, 0x00, 0x00, 0x00, 
                                0xDE, 0xAD, 0xBE, 0xEF, 
                                0xFF                    
                            ];
                            let _ = socket.send_to(&dummy_rtp, target).await;
                        }
                    }
                },

                Some((data, addr)) = rtp_packet_rx.recv() => {
                    last_activity = Instant::now();
                    total_packets_rx += 1;
                    if endpoint.latch(addr) { 
                        info!(
                            event = "RTP_LATCH_LOCKED", 
                            sip.call_id = %self.call_id,
                            peer.ip = %addr.ip(), 
                            peer.port = addr.port(), 
                            "🔒 Medya Hedefi Kilitlendi"
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
                         &mut is_playing, &mut echo_mode, &session_config, 
                         &socket, &finished_tx, &mut known_target, &endpoint, &self.call_id
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
            match crate::rtp::session_utils::finalize_and_save_recording(rec, self.app_state.clone()).await {
                Ok(_) => info!(event="RECORDING_SAVED", sip.call_id=%self.call_id, "💾 Kayıt başarıyla S3'e yüklendi."),
                Err(e) => error!(event="RECORDING_FAIL", sip.call_id=%self.call_id, error=%e, "❌ Kayıt S3'e yazılamadı!"),
            }
        }
        
        self.app_state.port_manager.remove_session(self.port).await;
        self.app_state.port_manager.quarantine_port(self.port).await;
        
        gauge!(ACTIVE_SESSIONS).decrement(1.0);
        
        info!(event = "RTP_SESSION_END", sip.call_id = %self.call_id, "🛑 RTP Oturumu Sonlandırıldı.");
    }
}