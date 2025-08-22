// ========== FILE: sentiric-media-service/src/audio.rs (Nihai Akıllı Sürüm) ==========
use std::collections::HashMap;
use std::path::PathBuf; // PathBuf'ı import et
use std::sync::Arc;
use anyhow::{Context, Result};
use tokio::sync::Mutex;
use tracing::{debug, info};

pub type AudioCache = Arc<Mutex<HashMap<String, Arc<Vec<i16>>>>>;

// DİKKAT: Fonksiyon imzası artık PathBuf alıyor
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

    // hound kütüphanesi senkron olduğu için, IO yoğun işlemi blocking task'e taşıyalım.
    let path_owned = audio_path.clone(); // Klonla çünkü closure'a taşıyacağız
    let new_samples = tokio::task::spawn_blocking(move || {
        hound::WavReader::open(path_owned)?.samples::<i16>().collect::<Result<Vec<_>, _>>()
    }).await??;
    
    let new_samples_arc = Arc::new(new_samples);
    cache_guard.insert(path_key, new_samples_arc.clone());
    
    Ok(new_samples_arc)
}

// ... (linear_to_ulaw fonksiyonu ve ULAW_TABLE aynı kalacak)
const BIAS: i16 = 0x84;
static ULAW_TABLE: [u8; 256] = [
    0, 0, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3, 3, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4,
    5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5, 5,
    6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6,
    6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6,
    7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
    7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
    7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
    7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7
];
pub fn linear_to_ulaw(mut pcm_val: i16) -> u8 {
    let sign = if pcm_val < 0 { 0x80 } else { 0 };
    if sign != 0 { pcm_val = -pcm_val; }
    pcm_val = pcm_val.min(32635);
    pcm_val += BIAS;
    let exponent = ULAW_TABLE[((pcm_val >> 7) & 0xFF) as usize];
    let mantissa = (pcm_val >> (exponent as i16 + 3)) & 0xF;
    !(sign as u8 | (exponent << 4) | mantissa as u8)
}