use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use anyhow::{Context, Result};
use tokio::sync::Mutex;
use tracing::{debug, info};

pub type AudioCache = Arc<Mutex<HashMap<String, Arc<Vec<i16>>>>>;

pub async fn load_or_get_from_cache(
    cache: &AudioCache, 
    audio_id: &str,
    assets_base_path: &str
) -> Result<Arc<Vec<i16>>> {
    let mut cache_guard = cache.lock().await;
    if let Some(cached_samples) = cache_guard.get(audio_id) {
        debug!("Ses dosyası önbellekten okundu.");
        return Ok(cached_samples.clone());
    }

    info!("Ses dosyası diskten okunuyor ve önbelleğe alınıyor.");
    
    // --- YENİ MANTIK BURADA ---
    // Eğer audio_id mutlak bir yolsa (/ ile başlıyorsa) onu kullan,
    // değilse, base path'e göre birleştir.
    let final_path = if audio_id.starts_with('/') {
        PathBuf::from(audio_id)
    } else {
        let mut path = PathBuf::from(assets_base_path);
        path.push(audio_id);
        path
    };
    // --- BİTTİ ---

    let path_str = final_path.to_str().context("Geçersiz dosya yolu karakterleri")?;
    debug!(path = path_str, "Oluşturulan dosya yolu.");

    let audio_id_owned = audio_id.to_string();
    let new_samples = tokio::task::spawn_blocking(move || {
        hound::WavReader::open(final_path)?.samples::<i16>().collect::<Result<Vec<_>, _>>()
    }).await??;
    
    let new_samples_arc = Arc::new(new_samples);
    cache_guard.insert(audio_id_owned, new_samples_arc.clone());
    
    Ok(new_samples_arc)
}

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