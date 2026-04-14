// sentiric-media-service/src/rabbitmq.rs
use lapin::{
    options::*, types::FieldTable, BasicProperties, Channel as LapinChannel, Connection,
    ConnectionProperties, ExchangeKind,
};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

pub const EXCHANGE_NAME: &str = "sentiric_events";

// [ARCH-COMPLIANCE FIX] Ghost Publisher
pub struct RabbitMqClient {
    channel: Arc<RwLock<Option<LapinChannel>>>,
}

impl RabbitMqClient {
    pub async fn new(url: &str) -> Self {
        let client = Self {
            channel: Arc::new(RwLock::new(None)),
        };

        let chan_clone = client.channel.clone();
        let url_clone = url.to_string();

        tokio::spawn(async move {
            let mut attempt = 0;
            loop {
                attempt += 1;
                if attempt == 1 {
                    info!(
                        event = "RABBITMQ_CONNECTING",
                        "🐇 RabbitMQ'ya bağlanılıyor..."
                    );
                }

                match Connection::connect(&url_clone, ConnectionProperties::default()).await {
                    Ok(conn) => match conn.create_channel().await {
                        Ok(channel) => {
                            let _ = channel
                                .exchange_declare(
                                    EXCHANGE_NAME,
                                    ExchangeKind::Topic,
                                    ExchangeDeclareOptions {
                                        durable: true,
                                        ..Default::default()
                                    },
                                    FieldTable::default(),
                                )
                                .await;

                            *chan_clone.write().await = Some(channel);
                            info!(
                                event = "RABBITMQ_RECOVERED",
                                "✅ [MQ] RabbitMQ bağlantısı sağlandı."
                            );
                            break;
                        }
                        Err(e) => {
                            error!(event = "RABBITMQ_CHANNEL_FAIL", error = %e, "❌ RabbitMQ kanalı oluşturulamadı.")
                        }
                    },
                    Err(e) => {
                        if attempt == 1 {
                            warn!(event = "RABBITMQ_UNREACHABLE", error = %e, "⚠️ RabbitMQ yok. Media Service Ghost Publisher modunda.");
                        } else {
                            tracing::debug!(
                                event = "RABBITMQ_RETRY",
                                attempt = attempt,
                                "RabbitMQ bekleniyor..."
                            );
                        }
                    }
                }
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            }
        });

        client
    }

    pub async fn publish_with_confirm(
        &self,
        routing_key: &str,
        payload: &[u8],
    ) -> anyhow::Result<()> {
        let channel_guard = self.channel.read().await;
        if let Some(channel) = channel_guard.as_ref() {
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
                anyhow::bail!("RabbitMQ mesajı NACK etti");
            }
            Ok(())
        } else {
            tracing::debug!(event="GHOST_PUBLISH", routing_key=%routing_key, "RabbitMQ yok, mesaj yutuldu (Ghost Mode).");
            Ok(())
        }
    }
}
