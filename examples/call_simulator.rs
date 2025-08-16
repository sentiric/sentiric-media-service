// examples/call_simulator.rs

use anyhow::Result;
use rand::seq::SliceRandom;
use rand::Rng;
use std::env;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::sleep;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};

// Gerekli gRPC tiplerini import ediyoruz.
use sentiric_contracts::sentiric::media::v1::{
    media_service_client::MediaServiceClient, AllocatePortRequest, PlayAudioRequest,
    ReleasePortRequest,
};

// --- SİMÜLASYON KONFİGÜRASYONU ---

// Senaryo Tipleri: Simülatörün hangi davranışları sergileyebileceğini tanımlar.
#[derive(Debug, Clone, Copy)]
enum Scenario {
    AgentCall,      // Standart bir ajan görüşmesi (uzun süreli)
    PlayIvrMenu,    // Müşteriye IVR menüsü çalma (kısa süreli, anonslu)
    LeaveVoicemail, // Sesli posta bırakma (orta süreli, gelecekte record API'si için yer)
}

// Toplamda kaç "işçi" (ajan/sistem süreci) çalışacak?
const TOTAL_WORKERS: usize = 20;

// Her bir işçi toplamda kaç görev (çağrı/anons) yapacak?
const TASKS_PER_WORKER: usize = 5;

// Senaryoların dağılımı. Toplamı 100 olmalı.
const SCENARIO_DISTRIBUTION: &[(Scenario, u32)] = &[
    (Scenario::AgentCall, 70),      // Çağrıların %70'i normal ajan görüşmesi
    (Scenario::PlayIvrMenu, 25),    // %25'i IVR menüsü çalma
    (Scenario::LeaveVoicemail, 5), // %5'i sesli posta bırakma
];

// --- Senaryo Zamanlama Parametreleri (Saniye Cinsinden) ---
const AGENT_CALL_DURATION: (u64, u64) = (20, 90); // 20sn - 1.5dk
const IVR_MENU_DURATION: (u64, u64) = (5, 15); // 5sn - 15sn
const VOICEMAIL_DURATION: (u64, u64) = (10, 40); // 10sn - 40sn

// Bir görev bittikten sonra bir sonrakine geçmeden önceki bekleme süresi
const POST_TASK_WAIT: (u64, u64) = (5, 15);

// --- Diğer Ayarlar ---
// Gerçekçi telefon numaraları için alan kodları
const TURKEY_AREA_CODES: &[&str] = &["505", "506", "507", "532", "533", "535", "542", "544", "555"];

// --- Kod Başlangıcı ---

#[derive(Clone)]
struct SimulatorConfig {
    server_addr: String,
    tls_config: ClientTlsConfig,
}

struct SimStats {
    tasks_started: AtomicUsize,
    tasks_failed_to_start: AtomicUsize,
    agent_calls: AtomicUsize,
    ivr_plays: AtomicUsize,
    voicemails: AtomicUsize,
}

impl SimStats {
    fn new() -> Self {
        Self {
            tasks_started: AtomicUsize::new(0),
            tasks_failed_to_start: AtomicUsize::new(0),
            agent_calls: AtomicUsize::new(0),
            ivr_plays: AtomicUsize::new(0),
            voicemails: AtomicUsize::new(0),
        }
    }
}

fn generate_phone_number() -> String {
    let mut rng = rand::thread_rng();
    let area_code = TURKEY_AREA_CODES.choose(&mut rng).unwrap();
    let subscriber_number: u32 = rng.gen_range(1_000_000..10_000_000);
    format!("90{}{}", area_code, subscriber_number)
}

fn choose_scenario() -> Scenario {
    let mut rng = rand::thread_rng();
    SCENARIO_DISTRIBUTION
        .choose_weighted(&mut rng, |item| item.1)
        .unwrap()
        .0
}

// Simülasyonu çalıştıran ana işçi fonksiyonu.
async fn run_worker(worker_id: usize, config: SimulatorConfig, stats: Arc<SimStats>) {
    println!("[Worker {}] Simülasyona başlıyor.", worker_id);
    let mut client = match Channel::from_shared(config.server_addr.clone())
        .unwrap().tls_config(config.tls_config).unwrap().connect().await {
        Ok(ch) => MediaServiceClient::new(ch),
        Err(e) => {
            eprintln!("[Worker {}] Bağlantı hatası: {}. Bu işçi sonlandırılıyor.", worker_id, e);
            stats.tasks_failed_to_start.fetch_add(TASKS_PER_WORKER, Ordering::SeqCst);
            return;
        }
    };

    for task_num in 0..TASKS_PER_WORKER {
        let scenario = choose_scenario();
        let from_number = generate_phone_number();
        let to_number = generate_phone_number();
        let call_id = format!("call-{}-{}", from_number, chrono::Utc::now().timestamp_millis());
        
        println!("[Worker {} | Görev {}] Senaryo: {:?}, Arayan: {}, Aranan: {}", worker_id, task_num + 1, scenario, from_number, to_number);
        
        let allocate_req = tonic::Request::new(AllocatePortRequest { call_id: call_id.clone() });
        let port = match client.allocate_port(allocate_req).await {
            Ok(res) => {
                stats.tasks_started.fetch_add(1, Ordering::SeqCst);
                res.into_inner().rtp_port
            }
            Err(e) => {
                eprintln!("[Worker {}] Port alınamadı, görev başarısız: {}", worker_id, e);
                stats.tasks_failed_to_start.fetch_add(1, Ordering::SeqCst);
                continue;
            }
        };

        let (duration_range, audio_id) = match scenario {
            Scenario::AgentCall => {
                stats.agent_calls.fetch_add(1, Ordering::SeqCst);
                (AGENT_CALL_DURATION, None)
            }
            Scenario::PlayIvrMenu => {
                stats.ivr_plays.fetch_add(1, Ordering::SeqCst);
                (IVR_MENU_DURATION, Some("audio/tr/welcome.wav"))
            }
            Scenario::LeaveVoicemail => {
                stats.voicemails.fetch_add(1, Ordering::SeqCst);
                (VOICEMAIL_DURATION, None)
            }
        };
        
        if let Some(audio) = audio_id {
            let play_req = tonic::Request::new(PlayAudioRequest {
                audio_id: audio.to_string(),
                server_rtp_port: port,
                rtp_target_addr: "127.0.0.1:30000".to_string(),
            });
            if let Err(e) = client.play_audio(play_req).await {
                eprintln!("[Worker {}] PlayAudio hatası: {}", worker_id, e);
            }
        }
        
        // --- DÜZELTİLMİŞ KISIM ---
        // Rastgele sayı üretecini (rng) kendi bloğu içinde oluşturup kullanıyoruz.
        // Bu, değişkenin 'Send' olmayan tipinin bir `.await` noktasından sonraya "taşınmasını" engeller.
        let duration = {
            let mut rng = rand::thread_rng();
            rng.gen_range(duration_range.0..=duration_range.1)
        };
        sleep(Duration::from_secs(duration)).await;

        let release_req = tonic::Request::new(ReleasePortRequest { rtp_port: port });
        if let Err(e) = client.release_port(release_req).await {
            eprintln!("[Worker {}] Port bırakma hatası: {}", worker_id, e);
        }
        println!("[Worker {}] Görev tamamlandı. (Süre: {}s)", worker_id, duration);

        if task_num < TASKS_PER_WORKER - 1 {
            // Aynı şekilde, bekleme süresi için de 'rng'yi kendi bloğunda oluşturuyoruz.
            let wait_time = {
                let mut rng = rand::thread_rng();
                rng.gen_range(POST_TASK_WAIT.0..=POST_TASK_WAIT.1)
            };
            println!("[Worker {}] Bir sonraki görev için {} saniye bekliyor...", worker_id, wait_time);
            sleep(Duration::from_secs(wait_time)).await;
        }
    }
    println!("[Worker {}] Simülasyonu tamamladı.", worker_id);
}


#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    // rand ve chrono'nun Cargo.toml'da olduğundan emin olun
    // [dependencies]
    // rand = "0.8.5"
    // chrono = "0.4.31"
    
    // Senaryo dağılımının 100 olduğundan emin olalım
    let total_distribution: u32 = SCENARIO_DISTRIBUTION.iter().map(|&(_, weight)| weight).sum();
    if total_distribution != 100 {
        panic!("SCENARIO_DISTRIBUTION toplamı 100 olmalı, fakat {} bulundu.", total_distribution);
    }
    
    println!("--- Gelişmiş Çağrı Simülatörü Başlatılıyor ---");
    println!("Ayarlar: {} işçi, işçi başına {} görev.", TOTAL_WORKERS, TASKS_PER_WORKER);
    
    let client_cert_path = env::var("AGENT_SERVICE_CERT_PATH")?;
    let client_key_path = env::var("AGENT_SERVICE_KEY_PATH")?;
    let ca_path = env::var("GRPC_TLS_CA_PATH")?;
    let media_service_host = env::var("MEDIA_SERVICE_HOST")?;
    let media_service_url = env::var("MEDIA_SERVICE_GRPC_URL")?;
    let server_addr = format!("https://{}", media_service_url);

    let client_identity = Identity::from_pem(tokio::fs::read(&client_cert_path).await?, tokio::fs::read(&client_key_path).await?);
    let server_ca_certificate = Certificate::from_pem(tokio::fs::read(&ca_path).await?);
    let tls_config = ClientTlsConfig::new()
        .domain_name(media_service_host)
        .ca_certificate(server_ca_certificate)
        .identity(client_identity);
    
    let config = SimulatorConfig { server_addr, tls_config };
    let stats = Arc::new(SimStats::new());
    let mut handles = Vec::new();
    let start_time = Instant::now();

    for i in 0..TOTAL_WORKERS {
        handles.push(tokio::spawn(run_worker(i + 1, config.clone(), stats.clone())));
    }

    for handle in handles {
        handle.await?;
    }

    let duration = start_time.elapsed();
    let total_tasks = stats.tasks_started.load(Ordering::SeqCst) + stats.tasks_failed_to_start.load(Ordering::SeqCst);
    
    println!("\n--- Simülasyon Sonuçları ---");
    println!("Toplam Süre: {:?}", duration);
    if !duration.is_zero() {
        println!("Saniyedeki Ortalama Görev (TPS): {:.2}", total_tasks as f64 / duration.as_secs_f64());
    }
    
    println!("\n--- Görev İstatistikleri ---");
    println!("✅ Başarıyla Başlatılan Görevler: {}", stats.tasks_started.load(Ordering::SeqCst));
    println!("❌ Başlatılamayan Görevler: {}", stats.tasks_failed_to_start.load(Ordering::SeqCst));
    
    println!("\n--- Senaryo Dağılımı ---");
    println!("📞 Ajan Çağrıları: {}", stats.agent_calls.load(Ordering::SeqCst));
    println!("🎶 IVR Anonsları: {}", stats.ivr_plays.load(Ordering::SeqCst));
    println!("🎙️ Sesli Postalar: {}", stats.voicemails.load(Ordering::SeqCst));

    Ok(())
}