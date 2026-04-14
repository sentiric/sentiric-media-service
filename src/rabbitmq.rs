// Dosya: src/rabbitmq.rs
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
        if attempt == 1 {
            info!(
                event = "RABBITMQ_CONNECTING",
                "🐇 RabbitMQ'ya bağlanılıyor..."
            );
        }

        match Connection::connect(url, ConnectionProperties::default()).await {
            Ok(conn) => match conn.create_channel().await {
                Ok(channel) => {
                    if let Err(e) = channel
                        .confirm_select(ConfirmSelectOptions::default())
                        .await
                    {
                        error!(event = "RABBITMQ_CONFIRM_ERROR", error = %e, "🚨 RabbitMQ Confirm Mode Error");
                    }
                    conn.on_error(|err| error!(event = "RABBITMQ_CONN_ERROR", error = %err, "🚨 RabbitMQ Connection Error"));

                    if attempt > 1 {
                        info!(
                            event = "RABBITMQ_RECOVERED",
                            "✅ RabbitMQ bağlantısı sağlandı."
                        );
                    }
                    return Ok(Arc::new(channel));
                }
                Err(e) => {
                    error!(event = "RABBITMQ_CHANNEL_FAIL", error = %e, "❌ RabbitMQ kanalı oluşturulamadı.")
                }
            },
            Err(e) => {
                // [ARCH-COMPLIANCE FIX] SUTS v4.2: İlk hata WARN, sonrakiler DEBUG
                if attempt == 1 {
                    warn!(event = "RABBITMQ_UNREACHABLE", error = %e, "⚠️ RabbitMQ'ya ulaşılamıyor. Arka planda sessizce denenecek (Ghost Mode)...");
                } else {
                    tracing::debug!(
                        event = "RABBITMQ_RETRY",
                        attempt = attempt,
                        "RabbitMQ bekleniyor..."
                    );
                }
            }
        }
        sleep(Duration::from_secs(5)).await;
    }
}

pub async fn declare_exchange(channel: &LapinChannel) -> Result<(), lapin::Error> {
    channel
        .exchange_declare(
            EXCHANGE_NAME,
            ExchangeKind::Topic,
            ExchangeDeclareOptions {
                durable: true,
                ..Default::default()
            },
            FieldTable::default(),
        )
        .await
}

pub async fn publish_with_confirm(
    channel: &LapinChannel,
    routing_key: &str,
    payload: &[u8],
) -> anyhow::Result<()> {
    let confirm = channel
        .basic_publish(
            EXCHANGE_NAME,
            routing_key,
            BasicPublishOptions::default(),
            payload,
            BasicProperties::default().with_delivery_mode(2),
        )
        .await?
        .await?;

    if confirm.is_nack() {
        anyhow::bail!("RabbitMQ mesajı NACK etti (Disk dolu veya kota aşımı)");
    }
    Ok(())
}
