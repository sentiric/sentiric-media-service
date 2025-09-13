// examples\test.rs
use std::env;
use std::path::Path;

// Environment değişkenlerini kontrol etmek için makro
macro_rules! assert_env {
    ($var:expr) => {
        match env::var($var) {
            Ok(val) => {
                println!("✓ {}: {}", $var, if val.is_empty() { "(boş)" } else { &val });
            }
            Err(env::VarError::NotPresent) => {
                println!("⚠ {}: AYARLANMAMIŞ", $var);
            }
            Err(e) => {
                println!("⚠ {}: OKUMA HATASI - {}", $var, e);
            }
        }
    };
}

#[tokio::main]
async fn main() {
    // .env dosyasını yükle
    match dotenvy::from_filename(".env.example") {
        Ok(_) => println!("'.env.example' dosyası başarıyla yüklendi."),
        Err(e) => {
            panic!("'.env.example' dosyası yüklenemedi: {}", e);
        }
    };

    // Tüm environment değişkenlerini test et
    test_environment_variables();
    
    println!("Tüm environment değişkenleri başarıyla yüklendi ve test edildi!");
}

fn test_environment_variables() {
    // --- Temel Ortam Ayarları ---
    assert_env!("ENV_FILE_PATH");
    assert_env!("CONFIG_REPO_PATH");
    assert_env!("ENV");
    assert_env!("NODE_ENV");
    assert_env!("LOG_LEVEL");
    assert_env!("RUST_LOG");
    assert_env!("RUST_BACKTRACE");

    // --- Docker ve Docker Compose Bilgileri ---
    assert_env!("DOCKER_REGISTRY");
    assert_env!("TAG");
    assert_env!("NETWORK_NAME");
    assert_env!("COMPOSE_HTTP_TIMEOUT");

    // --- 12-Factor App Prensipleri ---
    assert_env!("KNOWLEDGE_SERVICE_DATA_PATH");

    // --- Sertifika Yolları ---
    assert_env!("GRPC_TLS_CA_PATH");

    // --- MinIO Ayarları ---
    assert_env!("MINIO_ROOT_USER");
    assert_env!("MINIO_ROOT_PASSWORD");
    assert_env!("MINIO_HOST");
    assert_env!("MINIO_API_PORT");
    assert_env!("MINIO_CONSOLE_PORT");

    // --- Media Servis Ayarları ---
    assert_env!("MEDIA_SERVICE_HOST");
    assert_env!("MEDIA_SERVICE_PORT");
    assert_env!("MEDIA_SERVICE_URL");
    assert_env!("MEDIA_SERVICE_GRPC_PORT");
    assert_env!("MEDIA_SERVICE_GRPC_URL");
    assert_env!("MEDIA_SERVICE_METRICS_PORT");

    assert_env!("SIP_GATEWAY_IPV4_EXTERNAL_ADDRESS");
    assert_env!("SIP_SIGNALING_IPV4_EXTERNAL_ADDRESS");    
    assert_env!("RTP_SERVICE_HOST");    
    assert_env!("DETECTED_IP");
    assert_env!("MEDIA_SERVICE_RECORD_BASE_PATH");

    // --- RTP Ayarları ---
    assert_env!("RTP_SERVICE_LISTEN_ADDRESS");
    assert_env!("RTP_SERVICE_PORT_QUARANTINE_SECONDS");
    assert_env!("RTP_SERVICE_PORT_MIN");
    assert_env!("RTP_SERVICE_PORT_MAX");

    // --- Asset Yolu ---
    assert_env!("ASSETS_BASE_PATH");

    // --- S3 Ayarları ---
    assert_env!("BUCKET_PUBLIC_BASE_URL");
    assert_env!("BUCKET_ENDPOINT_URL");
    assert_env!("BUCKET_REGION");
    assert_env!("BUCKET_NAME");
    assert_env!("BUCKET_ACCESS_KEY_ID");
    assert_env!("BUCKET_SECRET_ACCESS_KEY");
    assert_env!("BUCKET_TOKEN");

    // --- Sertifika Yolları (Detaylı) ---
    assert_env!("GRPC_TLS_CA_PATH");
    assert_env!("MEDIA_SERVICE_CERT_PATH");
    assert_env!("MEDIA_SERVICE_KEY_PATH");
    assert_env!("AGENT_SERVICE_CERT_PATH");
    assert_env!("AGENT_SERVICE_KEY_PATH");
    assert_env!("USER_SERVICE_CERT_PATH");
    assert_env!("USER_SERVICE_KEY_PATH");
    assert_env!("DIALPLAN_SERVICE_CERT_PATH");
    assert_env!("DIALPLAN_SERVICE_KEY_PATH");
    assert_env!("SIP_SIGNALING_CERT_PATH");
    assert_env!("SIP_SIGNALING_KEY_PATH");

    // Değerleri kontrol et (opsiyonel)
    check_certificate_paths();
    check_minio_settings();
    check_media_service_settings();

    println!("Tüm environment değişkenleri doğrulandı!");
}

fn check_certificate_paths() {
    let cert_paths = [
        "GRPC_TLS_CA_PATH",
        "MEDIA_SERVICE_CERT_PATH",
        "MEDIA_SERVICE_KEY_PATH",
        "AGENT_SERVICE_CERT_PATH",
        "AGENT_SERVICE_KEY_PATH",
        "USER_SERVICE_CERT_PATH",
        "USER_SERVICE_KEY_PATH",
        "DIALPLAN_SERVICE_CERT_PATH",
        "DIALPLAN_SERVICE_KEY_PATH",
        "SIP_SIGNALING_CERT_PATH",
        "SIP_SIGNALING_KEY_PATH",
    ];

    for path_var in cert_paths.iter() {
        if let Ok(path) = env::var(path_var) {
            if !path.is_empty() {
                println!("{}: {}", path_var, path);
                
                // Dosya var mı kontrol et (opsiyonel)
                if Path::new(&path).exists() {
                    println!("  ✓ Dosya mevcut");
                } else {
                    println!("  ⚠ Dosya bulunamadı: {}", path);
                }
            }
        }
    }
}

fn check_minio_settings() {
    println!("MinIO Ayarları:");
    println!("  Endpoint: {}", env::var("BUCKET_ENDPOINT_URL").unwrap_or_default());
    println!("  Bucket: {}", env::var("BUCKET_NAME").unwrap_or_default());
    println!("  Access Key: {}", env::var("BUCKET_ACCESS_KEY_ID").unwrap_or_default());
    
    let secret = env::var("BUCKET_SECRET_ACCESS_KEY").unwrap_or_default();
    println!("  Secret Key: {}", if secret.is_empty() { "AYARLANMAMIŞ" } else { "AYARLANDI" });
}

fn check_media_service_settings() {
    println!("Media Service Ayarları:");
    println!("  Host: {}", env::var("MEDIA_SERVICE_HOST").unwrap_or_default());
    println!("  Port: {}", env::var("MEDIA_SERVICE_HTTP_PORT").unwrap_or_default());
    println!("  GRPC Port: {}", env::var("MEDIA_SERVICE_GRPC_PORT").unwrap_or_default());
    println!("  Public IP: {}", env::var("DETECTED_IP").unwrap_or_default());
}

