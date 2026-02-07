// sentiric-media-service/src/audio.rs
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use anyhow::{Context, Result};
use tokio::sync::Mutex;

pub type AudioCache = Arc<Mutex<HashMap<String, Arc<Vec<i16>>>>>;

pub async fn load_or_get_from_cache(
    cache: &AudioCache, 
    audio_path: &PathBuf,
) -> Result<Arc<Vec<i16>>> {
    let path_key = audio_path.to_string_lossy().to_string();
    let mut cache_guard = cache.lock().await;

    if let Some(cached_samples) = cache_guard.get(&path_key) {
        return Ok(cached_samples.clone());
    }

    // DEĞİŞİKLİK: Unused variable uyarısı için _prefix eklendi
    let _path_str = audio_path.to_str().context("Geçersiz dosya yolu")?;
    let path_owned = audio_path.clone();
    
    let new_samples: Vec<i16> = tokio::task::spawn_blocking(move || -> Result<Vec<i16>> {
        let reader = hound::WavReader::open(path_owned).context("WAV açma hatası")?;
        let samples: Vec<i16> = reader.into_samples::<i16>()
            .map(|s| s.unwrap_or(0))
            .collect();
        Ok(samples)
    }).await.context("Thread join hatası")??;
    
    let arc_samples = Arc::new(new_samples);
    cache_guard.insert(path_key, arc_samples.clone());
    
    Ok(arc_samples)
}