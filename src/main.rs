use rustls::crypto::CryptoProvider;
use rustls::crypto::ring::default_provider; // Ring provider'ı import et
use sentiric_media_service::run;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Ring crypto provider'ı kullan
    let provider = default_provider();
    CryptoProvider::install_default(provider).expect("Failed to install crypto provider");
    
    run().await
}