use rustls::crypto::CryptoProvider;
use rustls::crypto::ring::default_provider;
use sentiric_media_service::app::App;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Kripto sağlayıcısını en başta kur.
    let provider = default_provider();
    CryptoProvider::install_default(provider).expect("Failed to install crypto provider");

    // Uygulamayı başlat ve çalıştır.
    App::bootstrap().await?.run().await
}