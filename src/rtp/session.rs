// sentiric-media-service/src/rtp/session.rs

use crate::metrics::ACTIVE_SESSIONS;
use crate::rtp::codecs::AudioCodec;
use crate::rtp::command::{RtpCommand, AudioFrame};
use crate::rtp::handlers::{self}; 
use crate::rtp::session_handlers::{PlaybackJob}; 
use crate::rtp::processing::AudioProcessor;
use crate::rtp::session_utils::finalize_and_save_recording;
use crate::state::AppState;
use crate::config::AppConfig;
use crate::utils::send_hole_punch_packet; // Bu import kalmalı, HolePunching komutunda kullanılıyor.

use metrics::gauge;
// use rand::Rng; // KALDIRILDI
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio::task;
use tokio::time::{Duration, Instant};

use tracing::{info, instrument, warn, error};

use sentiric_rtp_core::{
    CodecType, RtpHeader, RtpPacket, RtcpPacket, RtpEndpoint, Pacer, 
};


const MAX_SILENCE_PACKETS: usize = 50;
const RTCP_INTERVAL: Duration = Duration::from_secs(5);

// RTP Oturumunun Dışarıdan Okunabilir Konfigürasyonu
#[derive(Clone)]
pub struct RtpSessionConfig {
    pub app_state: AppState,
    pub app_config: Arc<AppConfig>,
    pub port: u16,
}

/// Bir RTP oturumunun tüm durumunu ve mantığını kapsülleyen ana yapı.
pub struct RtpSession {
    pub call_id: String,
    pub port: u16,
    command_tx: mpsc::Sender<RtpCommand>,
    app_state: AppState,
}

impl RtpSession {

    pub fn new(call_id: String, port: u16, socket: Arc<tokio::net::UdpSocket>, app_state: AppState) -> Arc<Self> {
        let (command_tx, command_rx) = mpsc::channel(app_state.port_manager.config.rtp_command_channel_buffer);
        
        let session = Arc::new(Self {
            call_id,
            port,
            command_tx,
            app_state: app_state.clone(),
        });
        
        tokio::spawn(Self::run(session.clone(), socket, command_rx));
        
        session
    }

    pub async fn send_command(&self, command: RtpCommand) -> Result<(), mpsc::error::SendError<RtpCommand>> {
        self.command_tx.send(command).await
    }
    
    /// Oturumun ana döngüsü. Tüm I/O ve işleme burada gerçekleşir.
    #[instrument(skip_all, fields(rtp_port = self.port, call_id = %self.call_id))]
    async fn run(self: Arc<Self>, socket: Arc<tokio::net::UdpSocket>, mut command_rx: mpsc::Receiver<RtpCommand>) {
        info!("🎧 RTP OTURUMU BAŞLADI (OOP Model).");

        let live_stream_sender = Arc::new(Mutex::new(None));
        let permanent_recording_session = Arc::new(Mutex::new(None));
        let endpoint = RtpEndpoint::new(None);
        
        let mut known_target_addr: Option<SocketAddr> = None; 

        let mut audio_processor = AudioProcessor::new(CodecType::PCMU); 

        let rtp_ssrc: u32 = rand::Rng::gen(&mut rand::thread_rng());
        let mut rtp_seq: u16 = rand::Rng::gen(&mut rand::thread_rng());
        let mut rtp_ts: u32 = rand::Rng::gen(&mut rand::thread_rng());

        let mut outbound_stream_rx: Option<mpsc::Receiver<Vec<u8>>> = None;
        let mut is_streaming_active = false;
        let mut silence_counter = 0;

        let mut playback_queue: VecDeque<PlaybackJob> = VecDeque::new();
        let mut is_playing_file = false;
        let (playback_finished_tx, mut playback_finished_rx) = mpsc::channel::<()>(1);

        let (rtp_packet_tx, mut rtp_packet_rx) = mpsc::channel::<(Vec<u8>, SocketAddr)>(256);
        let socket_reader_handle = task::spawn({
            let socket = socket.clone();
            async move {
                let mut buf = [0u8; 2048];
                loop {
                    if let Ok((len, remote_addr)) = socket.recv_from(&mut buf).await {
                        if len >= 12 && !buf[..len].iter().all(|&b| b == 0 || b == 10 || b == 13) {
                            if rtp_packet_tx.send((buf[..len].to_vec(), remote_addr)).await.is_err() { break; }
                        }
                    } else { break; }
                }
            }
        });

        let mut pacer = Pacer::new(Duration::from_millis(20));
        let mut last_rtcp = Instant::now();
        let mut last_activity = Instant::now();
        let inactivity_limit = self.app_state.port_manager.config.rtp_session_inactivity_timeout;
        
        let get_session_config = || RtpSessionConfig {
            app_state: self.app_state.clone(),
            app_config: self.app_state.port_manager.config.clone(),
            port: self.port,
        };
        
        loop {
            pacer.wait();

            if last_rtcp.elapsed() >= RTCP_INTERVAL {
                if let Some(mut target_addr) = endpoint.get_target().or(known_target_addr) {
                    target_addr.set_port(target_addr.port().wrapping_add(1));
                    let rtcp = RtcpPacket::new_sender_report(rtp_ssrc);
                    let _ = socket.send_to(&rtcp.to_bytes(), target_addr).await;
                }
                last_rtcp = Instant::now();
            }

            if last_activity.elapsed() > inactivity_limit {
                warn!("⏳ İnaktivite Zaman Aşımı. Oturum Kapatılıyor.");
                break;
            }

            if is_streaming_active {
                if let Some(target_addr) = endpoint.get_target().or(known_target_addr) {
                    if let Some(rx) = &mut outbound_stream_rx {
                        while let Ok(chunk) = rx.try_recv() {
                            audio_processor.push_data(chunk);
                        }
                    }
                    if let Some(packets) = audio_processor.process_frame().await {
                        silence_counter = 0;
                        for payload in packets {
                            send_rtp_packet(&socket, target_addr, payload, &mut rtp_seq, &mut rtp_ts, rtp_ssrc, audio_processor.get_current_codec()).await;
                        }
                    } else if silence_counter < MAX_SILENCE_PACKETS {
                        let payload = audio_processor.generate_silence();
                        send_rtp_packet(&socket, target_addr, payload, &mut rtp_seq, &mut rtp_ts, rtp_ssrc, audio_processor.get_current_codec()).await;
                        silence_counter += 1;
                    }
                }
            }

            tokio::select! {
                biased;
                Some(command) = command_rx.recv() => {
                    last_activity = Instant::now();
                    
                    match command {
                        // Yeni komutlar burada işlenir
                        RtpCommand::SetTargetAddress { target } => {
                            info!("🎯 Hedef Adres Güncellendi: {}", target);
                            known_target_addr = Some(target);
                        },
                        RtpCommand::HolePunching { target_addr } => {
                            info!("🔨 Hole Punching Başlatıldı -> {}", target_addr);
                            crate::utils::send_hole_punch_packet(&socket, target_addr, 5).await;
                        },
                        // Diğer komutlar session_handlers'a devredilir
                        _ => {
                            if handlers::handle_command(
                                command, self.port, &live_stream_sender, &permanent_recording_session, 
                                &mut outbound_stream_rx, &mut is_streaming_active, &mut playback_queue, 
                                &mut is_playing_file, &get_session_config(), &socket, &playback_finished_tx,
                                &mut known_target_addr, &endpoint
                            ).await { break; }
                        }
                    }
                },
                
                Some((packet_data, remote_addr)) = rtp_packet_rx.recv() => {
                    last_activity = Instant::now();
                    
                    if endpoint.latch(remote_addr) { 
                        info!("🔒 RTP Latch Başarılı. RTP hedefi güncellendi: {}", remote_addr);
                        // Latching olduğunda, bilinen hedef adresi de güncellenir
                        known_target_addr = Some(remote_addr); 
                    }
                    
                    let pt = packet_data[1] & 0x7F;
                    let payload = &packet_data[12..];
                    if let Ok(codec) = AudioCodec::from_rtp_payload_type(pt) {
                        audio_processor.update_codec(codec.to_core_type());
                        if let Ok(samples_16k) = crate::rtp::codecs::decode_rtp_to_lpcm16(payload, codec) {
                            if let Some(sender) = &*live_stream_sender.lock().await {
                                if !sender.is_closed() {
                                    let mut bytes = Vec::with_capacity(samples_16k.len() * 2);
                                    for sample in &samples_16k { bytes.extend_from_slice(&sample.to_le_bytes()); }
                                    let frame = AudioFrame { data: bytes.into(), media_type: "audio/L16;rate=16000".to_string() };
                                    let _ = sender.try_send(Ok(frame));
                                }
                            }
                            if let Some(session) = &mut *permanent_recording_session.lock().await {
                                session.mixed_samples_16khz.extend_from_slice(&samples_16k);
                            }
                        }
                    }
                },
                
                Some(_) = playback_finished_rx.recv() => {
                    last_activity = Instant::now();
                    is_playing_file = false;
                    if let Some(next_job) = playback_queue.pop_front() {
                        is_playing_file = true;
                        handlers::start_playback(next_job, &get_session_config(), socket.clone(), playback_finished_tx.clone()).await;
                    }
                },
                
                else => {} 
            }
        }
        
        socket_reader_handle.abort(); 
        if let Some(session) = permanent_recording_session.lock().await.take() {
            if let Err(e) = finalize_and_save_recording(session, self.app_state.clone()).await {
                error!("CRITICAL: Recording save failed! Error: {}", e);
            }
        }

        self.app_state.port_manager.remove_session(self.port).await;
        self.app_state.port_manager.quarantine_port(self.port).await;
        gauge!(ACTIVE_SESSIONS).decrement(1.0);
        info!("🏁 RTP Oturumu Sonlandı (Port: {})", self.port);
    }
}

async fn send_rtp_packet(
    socket: &tokio::net::UdpSocket, 
    target: SocketAddr, 
    payload: Vec<u8>,
    seq: &mut u16, 
    ts: &mut u32, 
    ssrc: u32, 
    codec: CodecType
) {
    let pt = match codec { CodecType::PCMU => 0, CodecType::PCMA => 8, CodecType::G729 => 18, CodecType::G722 => 9 };
    let header = RtpHeader::new(pt, *seq, *ts, ssrc);
    let packet = RtpPacket { header, payload };
    let _ = socket.send_to(&packet.to_bytes(), target).await;
    
    *seq = seq.wrapping_add(1);
    let ts_increment = if codec.sample_rate() == 16000 { 320 } else { 160 }; 
    *ts = ts.wrapping_add(ts_increment);
}