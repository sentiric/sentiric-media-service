// Mevcut importlarınız
use sentiric_media_service::run;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // --- YENİ KOD BAŞLANGICI ---
    // Bu, projedeki tüm rustls kullanımları için 'ring' kripto sağlayıcısını
    // varsayılan olarak ayarlar. Bu, aws-sdk-s3 ve tonic arasındaki
    // olası çakışmaları çözer.
    // Bu satır, herhangi bir TLS işlemi (örn: S3 veya gRPC client/server oluşturma)
    // başlamadan ÖNCE, main fonksiyonunun en başında olmalıdır.
    if let Err(e) = rustls::crypto::ring::install_default_provider() {
        // Bu hata genellikle sadece bir sağlayıcı zaten yüklenmişse oluşur,
        // bu yüzden genellikle görmezden gelinebilir, ancak biz loglayalım.
        eprintln!("Could not install default ring crypto provider: {:?}", e);
    }
    // --- YENİ KOD SONU ---

    // Geri kalan kodunuz aynı
    run().await
}