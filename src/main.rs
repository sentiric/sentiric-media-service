use sentiric_media_service::run;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // --- NİHAİ DÜZELTME BAŞLANGICI ---
    // Bu, rustls'in 0.22 ve 0.23 versiyonlarında çalışan,
    // varsayılan kripto sağlayıcısını ayarlamanın standart ve
    // en doğru yoludur.
    rustls::crypto::CryptoProvider::install_default(
        rustls::crypto::ring::default_provider()
    );
    // --- NİHAİ DÜZELTME SONU ---
    
    // Eski hatalı kodu tamamen siliyoruz:
    // if let Err(e) = rustls::crypto::ring::install_default_provider() { ... }

    // Geri kalan kodunuz aynı
    run().await
}