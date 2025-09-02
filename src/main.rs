use sentiric_media_service::run;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // --- NİHAİ VE VERSİYON UYUMLU DÜZELTME ---
    // rustls v0.22.x, varsayılan sağlayıcıyı ayarlamak için `set_default` metodunu kullanır.
    // Bu, `install_default`'ın eski versiyonlardaki karşılığıdır.
    rustls::crypto::CryptoProvider::set_default(
        rustls::crypto::ring::default_provider()
    );
    // --- DÜZELTME SONU ---
    
    run().await
}