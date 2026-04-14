// Dosya: sentiric-media-service/src/rtp/session.rs
use crate::config::AppConfig;
use crate::rtp::codecs::AudioCodec;
use crate::rtp::command::{RecordingSession, RtpCommand};
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
    AudioProfile, CodecFactory, Decoder, Encoder, RtpEndpoint, RtpHeader, RtpPacket,
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

        let version = (data[0] >> 6) & 0x03;
        if version != 2 {
            return None;
        }

        let padding = (data[0] >> 5) & 0x01 != 0;
        let extension = (data[0] >> 4) & 0x01 != 0;
        let csrc_count = data[0] & 0x0F;

        let header = RtpHeader {
            version,
            padding,
            extension,
            csrc_count,
            marker: (data[1] >> 7) & 0x01 != 0,
            payload_type: data[1] & 0x7F,
            sequence_number: u16::from_be_bytes([data[2], data[3]]),
            timestamp: u32::from_be_bytes([data[4], data[5], data[6], data[7]]),
            ssrc: u32::from_be_bytes([data[8], data[9], data[10], data[11]]),
        };

        // [ARCH-COMPLIANCE] RFC 3550: Calculate exact payload offset skipping CSRCs and Extensions
        let mut offset = 12 + (csrc_count as usize * 4);
        if data.len() < offset {
            return None;
        }

        if extension {
            if data.len() < offset + 4 {
                return None;
            }
            let ext_len = u16::from_be_bytes([data[offset + 2], data[offset + 3]]) as usize * 4;
            offset += 4 + ext_len;
        }

        if data.len() < offset {
            return None;
        }

        let mut end_offset = data.len();
        if padding {
            let pad_len = data[data.len() - 1] as usize;
            if pad_len > 0 && offset + pad_len <= data.len() {
                end_offset -= pad_len;
            }
        }

        let payload = data[offset..end_offset].to_vec();
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
            "🎧 RTP Oturumu Başlatıldı (Fluid Ingress Queue Devrede)"
        );

        let live_stream_sender: crate::rtp::command::SharedLiveStreamSender =
            Arc::new(Mutex::new(None));
        let recording_session: Arc<Mutex<Option<RecordingSession>>> = Arc::new(Mutex::new(None));
        let endpoint = RtpEndpoint::new(None);

        let mut ingress_queue: VecDeque<i16> = VecDeque::with_capacity(32000);
        let mut egress_queue: VecDeque<i16> = VecDeque::with_capacity(32000);

        // [CRITICAL FIX] Jitter Buffer durum yöneticisi
        let mut is_buffering = true;

        // ... (packet_loss_count vb. tanımlamalar aynı kalacak) ...
        let packet_loss_count = 0u64; // mut kaldırılmıştı hatırlarsanız
        let mut total_packets_rx = 0u64;
        let mut echo_tx_count = 0u64;
        let mut known_target: Option<SocketAddr> = None;
        let mut echo_mode = false;

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
                        // [ARCH-COMPLIANCE] Baresip gibi istemcilerden sızan RTCP (192-205) paketlerini yoksay.
                        // Aksi halde ses decoder'ına girip kuyruğu bozabilir.
                        if packet.header.payload_type >= 192 && packet.header.payload_type <= 205 {
                            continue; // Bu paketi yoksay ve döngüye devam et
                        }

                        // Kodek Güncelleme
                        if active_payload_type != Some(packet.header.payload_type) {

                            if let Ok(codec) = AudioCodec::from_rtp_payload_type(packet.header.payload_type) {
                                active_decoder = Some(CodecFactory::create_decoder(codec.to_core_type()));
                                active_encoder = Some(CodecFactory::create_encoder(codec.to_core_type()));
                                active_payload_type = Some(packet.header.payload_type);
                            }
                        }

                        // Gelen paketi ANINDA çöz ve Ingress Kuyruğuna At!
                        if packet.header.payload_type != 101 {
                            if let Some(ref mut decoder) = active_decoder {
                                let raw_pcm = decoder.decode(&packet.payload);
                                if !raw_pcm.is_empty() {
                                    if (gain_multiplier - 1.0).abs() > f32::EPSILON {
                                        ingress_queue.extend(raw_pcm.into_iter().map(|s| {
                                            ((s as f32 * gain_multiplier) as i32).clamp(i16::MIN as i32, i16::MAX as i32) as i16
                                        }));
                                    } else {
                                        ingress_queue.extend(raw_pcm);
                                    }

                                    // [ARCH-COMPLIANCE FIX] OOM Protection & Jitter Reset
                                    if ingress_queue.len() > 16000 {
                                        tracing::warn!(event="INGRESS_QUEUE_OVERFLOW", sip.call_id=%self.call_id, "Ingress kuyruğu taştı, AI gecikmesi önleniyor (Catch-up).");
                                        let drain_count = ingress_queue.len() - 8000;
                                        ingress_queue.drain(0..drain_count);
                                    }
                                }
                            }
                        }
                    }
                },

                Some(pcm_data) = egress_rx.recv() => {
                    last_activity = Instant::now();
                    egress_queue.extend(pcm_data);

                    // [ARCH-COMPLIANCE FIX] OOM Protection (Max 2 saniyelik Egress Buffer = 16000 sample)
                    if egress_queue.len() > 16000 {
                        tracing::warn!(event="EGRESS_QUEUE_OVERFLOW", sip.call_id=%self.call_id, "Egress kuyruğu taştı, eski sesler kesiliyor (Catch-up).");
                        let drain_count = egress_queue.len() - 8000; // Yarısını tut, eskisini at
                        egress_queue.drain(0..drain_count);
                    }
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

                    // 1. INGRESS JITTER BUFFER (Müşteriden Gelen Sesi Çek)
                    if is_buffering {
                        // 4 paketlik (80ms) avans bekliyoruz
                        if ingress_queue.len() >= 640 {
                            is_buffering = false;
                        }
                    }

                    if !is_buffering {
                        if ingress_queue.len() >= 160 {
                            for item in rx_frame.iter_mut().take(160) {
                                *item = ingress_queue.pop_front().unwrap_or(0);
                            }
                            rx_has_audio = true;
                        } else {
                            // [CRITICAL FIX] Starvation (Kuyruk açlığı) oluştu.
                            // Cızırtı (Crackling) olmaması için sessizce Buffering moduna geri dön.
                            is_buffering = true;
                        }
                    }

                    // 2. SESİ DAĞIT (Echo ve AI için)
                    if rx_has_audio {
                        if let Some(tx) = &*live_stream_sender.lock().await {
                            let pcm_16k = sentiric_rtp_core::simple_resample(&rx_frame, 8000, 16000);
                            let mut b = Vec::with_capacity(pcm_16k.len() * 2);
                            for s in &pcm_16k { b.extend_from_slice(&s.to_le_bytes()); }
                            let _ = tx.try_send(Ok(crate::rtp::command::AudioFrame{ data: b.into(), media_type: "audio/L16;rate=16000".into() }));
                        }

                        if echo_mode {
                            egress_queue.extend(rx_frame.iter().copied());
                        }
                    }

                    // 3. EGRESS (Müşteriye Gidecek Sesi Hazırla)
                    if egress_queue.len() >= 160 {
                        for item in tx_frame.iter_mut().take(160) {
                            *item = egress_queue.pop_front().unwrap_or(0);
                        }
                    } else {
                        tx_frame.fill(0);
                    }

                    // 4. SESİ GÖNDER (Müşteriye)
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

                    // 5. S3 STEREO KAYIT
                    if let Some(rec) = &mut *recording_session.lock().await {
                        const MAX_SAMPLES: usize = 57_600_000;
                        if rec.rx_buffer.len() + 160 <= MAX_SAMPLES {
                            rec.rx_buffer.extend_from_slice(&rx_frame);
                            rec.tx_buffer.extend_from_slice(&tx_frame);
                            gauge!(RECORDING_BUFFER_BYTES).increment(640.0);
                        } else if !rec.max_reached_warned {
                            warn!(event = "MAX_RECORDING_REACHED", sip.call_id = %self.call_id, "OOM Koruması aktif.");
                            rec.max_reached_warned = true;
                        }
                    }
                },

                _ = stats_ticker.tick() => {
                    let loss_rate = if total_packets_rx > 0 { (packet_loss_count as f64 / (total_packets_rx + packet_loss_count) as f64) * 100.0 } else { 0.0 };
                    if total_packets_rx > 0 {
                        debug!(event = "RTP_QOS", sip.call_id = %self.call_id, loss_pct = loss_rate, rx = total_packets_rx, tx = echo_tx_count, "QoS Report");
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
