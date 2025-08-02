use sentiric_media_service::run;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Sadece kütüphanenin ana çalıştırma fonksiyonunu çağırır.
    run().await
}