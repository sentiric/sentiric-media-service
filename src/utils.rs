// sentiric-media-service/src/utils.rs

// Bu fonksiyon, bir URI string'inden şema kısmını ('file', 'data', 's3' vb.) ayıklar.
pub fn extract_uri_scheme(uri: &str) -> &str {
    if let Some(scheme_end) = uri.find(':') {
        &uri[..scheme_end]
    } else {
        // Şema bulunamazsa, belirsiz olarak işaretle. Bu, loglamada hatayı görmemizi sağlar.
        "unknown"
    }
}