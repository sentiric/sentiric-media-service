// Dosya: sentiric-media-service/src/rtp/session.rs
use crate::config::AppConfig;
use crate::rtp::codecs::AudioCodec;
use crate::rtp::command::{AudioFrame, RecordingSession, RtpCommand};
use crate::rtp::session_handlers;
use crate::state::AppState;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio::time::{Duration, Instant, MissedTickBehavior};
use tracing::{debug, error, info, instrument, warn};

use crate::metrics::{ACTIVE_SESSIONS, RECORDING_BUFFER_BYTES};
use metrics::gauge;

use sentiric_rtp_core::{
    AudioProfile, CodecFactory, Decoder, Encoder, JitterBuffer, RtpEndpoint, RtpHeader, RtpPacket,
};

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
    pub egress_tx: mpsc::Sender<Vec<i16>>,
    app_state: AppState,
}

impl RtpSession {
    pub fn new(
        trace_id: String,
        call_id: String,
        port: u16,
        socket: Arc<tokio::net::UdpSocket>,
        app_state: AppState,
    ) -> Arc<Self> {
        let (command_tx, command_rx) = mpsc::channel(128);
        let (egress_tx, egress_rx) = mpsc::channel(8192);

        let session = Arc::new(Self {
            call_id,
            trace_id,
            port,
            command_tx,
            egress_tx,
            app_state: app_state.clone(),
        });
        tokio::spawn(Self::run(session.clone(), socket, command_rx, egress_rx));
        session
    }

    pub async fn send_command(
        &self,
        command: RtpCommand,
    ) -> Result<(), mpsc::error::SendError<RtpCommand>> {
        self.command_tx.send(command).await
    }

    fn parse_rtp_packet(data: Vec<u8>) -> Option<RtpPacket> {
        if data.len() < 12 {
            return None;
        }
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
    async fn run(
        self: Arc<Self>,
        socket: Arc<tokio::net::UdpSocket>,
        mut command_rx: mpsc::Receiver<RtpCommand>,
        mut egress_rx: mpsc::Receiver<Vec<i16>>,
    ) {
        let gain_multiplier = self.app_state.port_manager.config.audio_recording_gain;

        info!(
            event = "RTP_SESSION_START",
            resource.service.name = "media-service",
            sip.call_id = %self.call_id,
            "🎧 RTP Oturumu Başlatıldı (Strict 20ms Sync Engine Devrede)"
        );

        let live_stream_sender: crate::rtp::command::SharedLiveStreamSender =
            Arc::new(Mutex::new(None));
        let recording_session: Arc<Mutex<Option<RecordingSession>>> = Arc::new(Mutex::new(None));
        let endpoint = RtpEndpoint::new(None);

        let mut jitter_buffer = JitterBuffer::new(100, 60);
        let mut egress_queue: VecDeque<i16> = VecDeque::with_capacity(16000);

        let mut last_seq: Option<u16> = None;
        let mut packet_loss_count = 0u64;
        let mut total_packets_rx = 0u64;
        let mut jitter_acc = 0.0f64;
        let mut last_arrival = Instant::now();

        let mut echo_mode = false;
        let mut echo_tx_count = 0u64;
        let mut known_target: Option<SocketAddr> = None;

        let server_ssrc: u32 = rand::random();
        let mut tx_seq: u16 = rand::random();
        let mut tx_ts: u32 = rand::random();

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

        let mut playback_queue: std::collections::VecDeque<session_handlers::PlaybackJob> =
            std::collections::VecDeque::new();
        let mut is_playing = false;
        let (finished_tx, mut finished_rx) = mpsc::channel(1);

        let mut stats_ticker = tokio::time::interval(Duration::from_secs(5));

        let mut ptime_ticker = tokio::time::interval(Duration::from_millis(20));
        ptime_ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

        let mut last_activity = Instant::now();

        let session_config = RtpSessionConfig {
            app_state: self.app_state.clone(),
            app_config: self.app_state.port_manager.config.clone(),
            port: self.port,
        };

        let default_profile = AudioProfile::default();
        let initial_codec = default_profile.preferred_audio_codec();

        let mut active_payload_type: Option<u8> = Some(initial_codec as u8);
        let mut active_decoder: Option<Box<dyn Decoder>> =
            Some(CodecFactory::create_decoder(initial_codec));
        let mut active_encoder: Option<Box<dyn Encoder>> =
            Some(CodecFactory::create_encoder(initial_codec));

        info!(event = "RTP_ENGINE_READY", codec = ?initial_codec, "🚀 Medya Motoru önbellekten yüklendi.");

        loop {
            let timeout = session_config.app_config.rtp_session_inactivity_timeout;

            if last_activity.elapsed() > timeout {
                warn!(
                    event = "RTP_INACTIVITY_WARNING",
                    sip.call_id = %self.call_id,
                    "⚠️ RTP trafiği alınamıyor. B2BUA gRPC emri bekleniyor."
                );
                last_activity = Instant::now();
            }

            tokio::select! {
                Some((data, addr)) = rtp_packet_rx.recv() => {
                    last_activity = Instant::now();
                    total_packets_rx += 1;
                    if endpoint.latch(addr) {
                        info!(event = "RTP_LATCH_LOCKED", sip.call_id = %self.call_id, peer.ip = %addr.ip(), peer.port = addr.port(), "🔒 Medya Hedefi Kilitlendi");
                    }

                    if let Some(packet) = Self::parse_rtp_packet(data) {
                        let now = Instant::now();
                        let delta = now.duration_since(last_arrival).as_millis() as f64;
                        jitter_acc += (delta - 20.0).abs();
                        last_arrival = now;

                        let seq = packet.header.sequence_number;
                        if let Some(prev) = last_seq {
                            let expected = prev.wrapping_add(1);
                            if seq != expected {
                                let diff = seq.wrapping_sub(expected);
                                if diff > 0 && diff < 1000 {
                                    packet_loss_count += diff as u64;
                                }
                            }
                        }
                        last_seq = Some(seq);

                        if active_payload_type != Some(packet.header.payload_type) {
                            if let Ok(codec) = AudioCodec::from_rtp_payload_type(packet.header.payload_type) {
                                active_decoder = Some(CodecFactory::create_decoder(codec.to_core_type()));
                                active_encoder = Some(CodecFactory::create_encoder(codec.to_core_type()));
                                active_payload_type = Some(packet.header.payload_type);
                            }
                        }
                        jitter_buffer.push(packet);
                    }
                },

                Some(pcm_data) = egress_rx.recv() => {
                    last_activity = Instant::now();
                    egress_queue.extend(pcm_data);
                },

                Some(cmd) = command_rx.recv() => {
                     last_activity = Instant::now();
                     if matches!(cmd, RtpCommand::Shutdown) { break; }

                     if session_handlers::handle_command(
                         cmd, &live_stream_sender, &recording_session,
                         &mut playback_queue, &mut is_playing, &mut echo_mode,
                         &session_config, &self.egress_tx, &finished_tx, &mut known_target, &endpoint, &self.call_id
                     ).await { break; }
                },

                _ = ptime_ticker.tick() => {
                    let mut rx_frame = vec![0i16; 160];
                    let mut tx_frame = vec![0i16; 160];
                    let mut rx_has_audio = false;
                    let mut decoded_samples = Vec::new();

                    // [CRITICAL FIX]: Gelen veri boyutu (160) kısıtlaması kaldırıldı.
                    // Decoder'ın döndüğü tüm ham ses tamponlanır ve işlenir.
                    if let Some(packet) = jitter_buffer.pop() {
                        if let Some(ref mut decoder) = active_decoder {
                            if packet.header.payload_type != 101 {
                                let raw_pcm = decoder.decode(&packet.payload);
                                if !raw_pcm.is_empty() {
                                    decoded_samples = raw_pcm;
                                    rx_has_audio = true;
                                } else {
                                    debug!(event="DECODE_EMPTY", pt=packet.header.payload_type, "Decoder returned empty PCM data");
                                }
                            }
                        }
                    }

                    // Sesi işleme ve dağıtma (Echo, Live Stream, Record)
                    if rx_has_audio {
                        // 1. Standart Frame (Kayıt ve Hizalama için ilk 160 sample)
                        let len_to_copy = decoded_samples.len().min(160);
                        if (gain_multiplier - 1.0).abs() > f32::EPSILON {
                            for i in 0..len_to_copy {
                                rx_frame[i] = ((decoded_samples[i] as f32 * gain_multiplier) as i32).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
                            }
                        } else {
                            rx_frame[..len_to_copy].copy_from_slice(&decoded_samples[..len_to_copy]);
                        }

                        // 2. AI Pipeline'a Canlı Akış (Tam Boyut)
                        if let Some(tx) = &*live_stream_sender.lock().await {
                            let pcm_16k = sentiric_rtp_core::simple_resample(&decoded_samples, 8000, 16000);
                            let mut b = Vec::with_capacity(pcm_16k.len() * 2);
                            for s in &pcm_16k { b.extend_from_slice(&s.to_le_bytes()); }
                            let _ = tx.try_send(Ok(crate::rtp::command::AudioFrame{ data: b.into(), media_type: "audio/L16;rate=16000".into() }));
                        }

                        // 3. Echo Test (Tam Boyut)
                        if echo_mode {
                            if (gain_multiplier - 1.0).abs() > f32::EPSILON {
                                egress_queue.extend(decoded_samples.iter().map(|&s| ((s as f32 * gain_multiplier) as i32).clamp(i16::MIN as i32, i16::MAX as i32) as i16));
                            } else {
                                egress_queue.extend(decoded_samples.iter().copied());
                            }
                        }
                    }

                    // TX Queue İşleme (Dışarıya basılacak ses)
                    if egress_queue.len() >= 160 {
                        for item in tx_frame.iter_mut().take(160) {
                            *item = egress_queue.pop_front().unwrap_or(0);
                        }
                    } else {
                        tx_frame.fill(0);
                    }

                    if let Some(target) = known_target.or_else(|| endpoint.get_target()) {
                        if let Some(enc) = &mut active_encoder {
                            let payload = enc.encode(&tx_frame);
                            let header = RtpHeader::new(enc.get_type() as u8, tx_seq, tx_ts, server_ssrc);
                            let packet = RtpPacket { header, payload };
                            let _ = socket.send_to(&packet.to_bytes(), target).await;

                            tx_seq = tx_seq.wrapping_add(1);
                            tx_ts = tx_ts.wrapping_add(160);
                            echo_tx_count += 1;
                        }
                    }

                    // 4. Stereo Kayıt Birleştirme (RX = Müşteri Sesi, TX = Sistem Sesi)
                    if let Some(rec) = &mut *recording_session.lock().await {
                        const MAX_SAMPLES: usize = 57_600_000;
                        if rec.rx_buffer.len() + 160 <= MAX_SAMPLES {
                            // Dinamik boyutlu sesin TAMAMINI kayıt bufferına ekle (Veri kaybını önler)
                            if rx_has_audio {
                                rec.rx_buffer.extend_from_slice(&decoded_samples);
                                // TX tarafını da RX boyutunda sıfırlarla uydur (Senkronizasyon için)
                                let mut tx_pad = vec![0i16; decoded_samples.len()];
                                let copy_len = tx_frame.len().min(decoded_samples.len());
                                tx_pad[..copy_len].copy_from_slice(&tx_frame[..copy_len]);
                                rec.tx_buffer.extend_from_slice(&tx_pad);
                                gauge!(RECORDING_BUFFER_BYTES).increment((decoded_samples.len() * 4) as f64);
                            } else {
                                rec.rx_buffer.extend_from_slice(&rx_frame);
                                rec.tx_buffer.extend_from_slice(&tx_frame);
                                gauge!(RECORDING_BUFFER_BYTES).increment(640.0);
                            }
                        } else if !rec.max_reached_warned {
                            warn!(event = "MAX_RECORDING_REACHED", sip.call_id = %self.call_id, "OOM Koruması aktif.");
                            rec.max_reached_warned = true;
                        }
                    }
                },

                _ = stats_ticker.tick() => {
                    let loss_rate = if total_packets_rx > 0 { (packet_loss_count as f64 / (total_packets_rx + packet_loss_count) as f64) * 100.0 } else { 0.0 };
                    let avg_jitter = if total_packets_rx > 0 { jitter_acc / total_packets_rx as f64 } else { 0.0 };
                    if total_packets_rx > 0 {
                        info!(event = "RTP_QOS", sip.call_id = %self.call_id, loss_pct = loss_rate, jitter_ms = avg_jitter, rx = total_packets_rx, tx = echo_tx_count, "QoS Report");
                    }
                },

                Some(_) = finished_rx.recv() => {
                     is_playing = false;
                     if let Some(next) = playback_queue.pop_front() {
                         is_playing = true;
                         session_handlers::start_playback(next, &session_config, self.egress_tx.clone(), finished_tx.clone(), &self.call_id).await;
                     }
                }
            }
        }

        if let Some(rec) = recording_session.lock().await.take() {
            match crate::rtp::session_utils::finalize_and_save_recording(
                rec,
                self.app_state.clone(),
            )
            .await
            {
                Ok(_) => {
                    info!(event="RECORDING_SAVED", sip.call_id=%self.call_id, "💾 Stereo kayıt başarıyla S3'e yüklendi.")
                }
                Err(e) => {
                    error!(event="RECORDING_FAIL", sip.call_id=%self.call_id, error=%e, "❌ Kayıt S3'e yazılamadı!")
                }
            }
        }

        self.app_state.port_manager.remove_session(self.port).await;
        self.app_state.port_manager.quarantine_port(self.port).await;

        gauge!(ACTIVE_SESSIONS).decrement(1.0);
        info!(event = "RTP_SESSION_END", sip.call_id = %self.call_id, "🛑 RTP Oturumu Sonlandırıldı.");
    }
}
