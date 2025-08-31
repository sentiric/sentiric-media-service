// File: sentiric-media-service/src/audio.rs (GÜNCELLENDİ)
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use anyhow::{Context, Result};
use tokio::sync::Mutex;
use tracing::{debug, info};

pub type AudioCache = Arc<Mutex<HashMap<String, Arc<Vec<i16>>>>>;

pub async fn load_or_get_from_cache(
    cache: &AudioCache, 
    audio_path: &PathBuf,
) -> Result<Arc<Vec<i16>>> {
    let path_key = audio_path.to_string_lossy().to_string();
    let mut cache_guard = cache.lock().await;

    if let Some(cached_samples) = cache_guard.get(&path_key) {
        debug!("Ses dosyası önbellekten okundu.");
        return Ok(cached_samples.clone());
    }

    info!("Ses dosyası diskten okunuyor ve önbelleğe alınıyor.");
    let path_str = audio_path.to_str().context("Geçersiz dosya yolu karakterleri")?;
    debug!(path = path_str, "Oluşturulan dosya yolu.");

    let path_owned = audio_path.clone();
    let new_samples = tokio::task::spawn_blocking(move || {
        hound::WavReader::open(path_owned)?.samples::<i16>().collect::<Result<Vec<_>, _>>()
    }).await??;
    
    let new_samples_arc = Arc::new(new_samples);
    cache_guard.insert(path_key, new_samples_arc.clone());
    
    Ok(new_samples_arc)
}

// NOT: G.711 dönüşüm fonksiyonları ve tabloları buradan kaldırılmıştır.
// Bu mantık artık merkezileştirilmiş olan `src/rtp/codecs.rs` dosyasında yer almaktadır.