use sentiric_media_service::run;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Tüm rustls ile ilgili manuel kurulum kodlarını sildik.
    // Sorunu Cargo.toml'da çözüyoruz.
    run().await
}