// sentiric-media-service/src/rabbitmq.rs
use lapin::{options::*, types::FieldTable, Channel as LapinChannel, Connection, ConnectionProperties, ExchangeKind};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{info, warn, error};

pub const EXCHANGE_NAME: &str = "sentiric_events";

/// RabbitMQ'ya bağlanır. Bağlantı koparsa veya kurulamazsa sonsuza kadar dener.
pub async fn connect_with_retry(url: &str) -> anyhow::Result<Arc<LapinChannel>> {
    let mut attempt = 0;
    
    loop {
        attempt += 1;
        info!("🐇 RabbitMQ'ya bağlanılıyor (Deneme: {})...", attempt);
        
        match Connection::connect(url, ConnectionProperties::default()).await {
            Ok(conn) => {
                match conn.create_channel().await {
                    Ok(channel) => {
                        info!("✅ RabbitMQ bağlantısı ve kanal başarıyla oluşturuldu.");
                        
                        // Bağlantı kopma durumunu logla
                        let _ = conn.on_error(|err| {
                            error!("🚨 RabbitMQ Connection Error: {}", err);
                        });

                        return Ok(Arc::new(channel));
                    },
                    Err(e) => {
                        error!("❌ RabbitMQ kanalı oluşturulamadı: {}. Tekrar deneniyor...", e);
                    }
                }
            },
            Err(e) => {
                warn!(
                    "⚠️ RabbitMQ'ya ulaşılamıyor (Deneme: {}): {}. 5 saniye sonra tekrar denenecek...",
                    attempt, e
                );
            }
        }
        
        // Altyapının toparlanması için bekle
        sleep(Duration::from_secs(5)).await;
    }
}

/// Standart Sentiric Exchange tanımlarını yapar.
pub async fn declare_exchange(channel: &LapinChannel) -> Result<(), lapin::Error> {
    info!("📢 Olay exchange'i tanımlanıyor: {}", EXCHANGE_NAME);
    channel
        .exchange_declare(
            EXCHANGE_NAME,
            ExchangeKind::Topic,
            ExchangeDeclareOptions {
                durable: true, // Mesaj kaybını önlemek için kalıcı
                ..Default::default()
            },
            FieldTable::default(),
        )
        .await
}