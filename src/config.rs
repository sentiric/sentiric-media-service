use std::env;
use std::net::SocketAddr;
use std::time::Duration;
use anyhow::{Result, Context, bail};

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
}

impl AppConfig {
    pub fn load_from_env() -> Result<Self> {
        let grpc_port: u16 = env::var("MEDIA_SERVICE_GRPC_PORT")
            .context("MEDIA_SERVICE_GRPC_PORT eksik")?
            .parse()?;
            
        let rtp_port_min: u16 = env::var("RTP_SERVICE_PORT_MIN")
            .context("RTP_SERVICE_PORT_MIN eksik")?
            .parse()?;
            
        let rtp_port_max: u16 = env::var("RTP_SERVICE_PORT_MAX")
            .context("RTP_SERVICE_PORT_MAX eksik")?
            .parse()?;
            
        // --- DÜZELTME BURADA ---
        // '...' yerine geçerli bir hata mesajı yazıyoruz.
        if rtp_port_min >= rtp_port_max {
            bail!("RTP port aralığı geçersiz: min ({}) >= max ({}).", rtp_port_min, rtp_port_max);
        }

        let quarantine_seconds: u64 = env::var("RTP_SERVICE_PORT_QUARANTINE_SECONDS")
            .unwrap_or_else(|_| "60".to_string())
            .parse()
            .context("RTP_SERVICE_PORT_QUARANTINE_SECONDS geçerli bir sayı olmalı")?;

        Ok(AppConfig {
            grpc_listen_addr: format!("[::]:{}", grpc_port).parse()?,
            rtp_host: env::var("RTP_SERVICE_LISTEN_ADDRESS").unwrap_or_else(|_| "0.0.0.0".to_string()),
            assets_base_path: env::var("ASSETS_BASE_PATH").unwrap_or_else(|_| "assets".to_string()),
            rtp_port_min,
            rtp_port_max,
            rtp_port_quarantine_duration: Duration::from_secs(quarantine_seconds),
            env: env::var("ENV").unwrap_or_else(|_| "production".to_string()),
            rust_log: env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string()),
        })
    }
}