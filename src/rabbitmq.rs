// sentiric-media-service/src/rabbitmq.rs
use lapin::{
    options::*, types::FieldTable, BasicProperties, Channel as LapinChannel, Connection,
    ConnectionProperties, ExchangeKind,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{error, info, warn};

pub const EXCHANGE_NAME: &str = "sentiric_events";

pub async fn connect_with_retry(url: &str) -> anyhow::Result<Arc<LapinChannel>> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        info!("🐇 RabbitMQ'ya bağlanılıyor (Deneme: {})...", attempt);
        match Connection::connect(url, ConnectionProperties::default()).await {
            Ok(conn) => {
                match conn.create_channel().await {
                    Ok(channel) => {
                        if let Err(e) = channel.confirm_select(ConfirmSelectOptions::default()).await {
                            error!("🚨 RabbitMQ Confirm Mode Error: {}", e);
                        }
                        
                        let _ = conn.on_error(|err| error!("🚨 RabbitMQ Connection Error: {}", err));
                        return Ok(Arc::new(channel));
                    },
                    Err(e) => error!("❌ RabbitMQ kanalı oluşturulamadı: {}. Tekrar deneniyor...", e)
                }
            },
            Err(e) => warn!("⚠️ RabbitMQ'ya ulaşılamıyor (Deneme: {}): {}. 5 saniye sonra tekrar denenecek...", attempt, e)
        }
        sleep(Duration::from_secs(5)).await;
    }
}

pub async fn declare_exchange(channel: &LapinChannel) -> Result<(), lapin::Error> {
    channel.exchange_declare(
        EXCHANGE_NAME,
        ExchangeKind::Topic,
        ExchangeDeclareOptions { durable: true, ..Default::default() },
        FieldTable::default(),
    ).await
}

pub async fn publish_with_confirm(channel: &LapinChannel, routing_key: &str, payload: &[u8]) -> anyhow::Result<()> {
    let confirm = channel.basic_publish(
        EXCHANGE_NAME,
        routing_key,
        BasicPublishOptions::default(),
        payload,
        BasicProperties::default().with_delivery_mode(2),
    ).await?.await?; 

    if confirm.is_nack() {
        anyhow::bail!("RabbitMQ mesajı NACK etti (Disk dolu veya kota aşımı)");
    }
    Ok(())
}