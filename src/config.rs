// sentiric-media-service/src/config.rs
use std::env;
use std::net::SocketAddr;
use std::time::Duration;
use anyhow::{Result, Context, bail};

#[derive(Debug, Clone, PartialEq)]
pub enum MediaEngineMode {
    Headless, // Sunucu modu (Ses kartı yok)
    Hardware, // Masaüstü/Lokal mod (Ses kartı var)
}

#[derive(Debug, Clone)]
pub struct S3Config {
    pub endpoint_url: String,
    pub region: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub bucket_name: String,
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub grpc_listen_addr: SocketAddr,
    pub rtp_host: String,
    pub rtp_port_min: u16,
    pub rtp_port_max: u16,
    pub rtp_port_quarantine_duration: Duration,
    pub assets_base_path: String,
    pub env: String,
    pub rust_log: String,
    pub log_format: String, // [YENİ]
    pub metrics_port: u16,
    pub s3_config: Option<S3Config>,
    pub rtp_session_inactivity_timeout: Duration,
    pub rtp_command_channel_buffer: usize,
    pub live_audio_stream_buffer: usize,
    pub rabbitmq_url: Option<String>,
    pub media_engine_mode: MediaEngineMode,
    pub cert_path: String,
    pub key_path: String,
    pub ca_path: String,
}

impl AppConfig {
    pub fn load_from_env() -> Result<Self> {
        let grpc_port: u16 = env::var("MEDIA_SERVICE_GRPC_PORT").context("MEDIA_SERVICE_GRPC_PORT eksik")?.parse()?;
        let metrics_port: u16 = env::var("MEDIA_SERVICE_METRICS_PORT").unwrap_or_else(|_| "13032".to_string()).parse()?;

        let rtp_port_min: u16 = env::var("RTP_SERVICE_PORT_MIN").unwrap_or_else(|_| "50000".to_string()).parse()?;
        let rtp_port_max: u16 = env::var("RTP_SERVICE_PORT_MAX").unwrap_or_else(|_| "50100".to_string()).parse()?;

        if rtp_port_min >= rtp_port_max { bail!("RTP port aralığı geçersiz."); }

        let quarantine_seconds: u64 = env::var("RTP_SERVICE_PORT_QUARANTINE_SECONDS").unwrap_or_else(|_| "5".to_string()).parse()?;
        let inactivity_seconds: u64 = env::var("RTP_SESSION_INACTIVITY_TIMEOUT_SECONDS").unwrap_or_else(|_| "30".to_string()).parse()?;
        
        let command_buffer: usize = 32;
        let stream_buffer: usize = 64;

        let s3_config = if env::var("BUCKET_ENDPOINT_URL").is_ok() {
            Some(S3Config {
                endpoint_url: env::var("BUCKET_ENDPOINT_URL")?,
                region: env::var("BUCKET_REGION")?,
                access_key_id: env::var("BUCKET_ACCESS_KEY_ID")?,
                secret_access_key: env::var("BUCKET_SECRET_ACCESS_KEY")?,
                bucket_name: env::var("BUCKET_NAME")?,
            })
        } else { None };
        
        let rabbitmq_url = env::var("RABBITMQ_URL").ok();

        let mode_str = env::var("MEDIA_ENGINE_MODE").unwrap_or_else(|_| "HEADLESS".to_string());
        let media_engine_mode = if mode_str.to_uppercase() == "HARDWARE" {
            MediaEngineMode::Hardware
        } else {
            MediaEngineMode::Headless
        };

        Ok(AppConfig {
            grpc_listen_addr: format!("[::]:{}", grpc_port).parse()?,
            rtp_host: env::var("RTP_SERVICE_LISTEN_ADDRESS").unwrap_or_else(|_| "0.0.0.0".to_string()),
            assets_base_path: env::var("ASSETS_BASE_PATH").unwrap_or_else(|_| "assets".to_string()),
            rtp_port_min,
            rtp_port_max,
            rtp_port_quarantine_duration: Duration::from_secs(quarantine_seconds),
            env: env::var("ENV").unwrap_or_else(|_| "production".to_string()),
            rust_log: env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string()),
            log_format: env::var("LOG_FORMAT").unwrap_or_else(|_| "text".to_string()), // [YENİ]
            metrics_port,
            s3_config,
            rabbitmq_url,
            rtp_session_inactivity_timeout: Duration::from_secs(inactivity_seconds),
            rtp_command_channel_buffer: command_buffer,
            live_audio_stream_buffer: stream_buffer,
            media_engine_mode,
            cert_path: env::var("MEDIA_SERVICE_CERT_PATH")?,
            key_path: env::var("MEDIA_SERVICE_KEY_PATH")?,
            ca_path: env::var("GRPC_TLS_CA_PATH")?,
        })
    }
}