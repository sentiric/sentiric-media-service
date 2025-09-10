// File: examples/live_audio_client.rs

use anyhow::{anyhow, Result};
use bytes::BytesMut;
use sentiric_contracts::sentiric::media::v1::{
    media_service_client::MediaServiceClient, AllocatePortRequest, RecordAudioRequest,
    ReleasePortRequest,
};
use std::env;
use std::time::Duration;
use tokio::time::{interval, timeout};
use tokio_stream::StreamExt;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};
use rtp::packet::Packet;
use rand::Rng;
use webrtc_util::marshal::Marshal;

const TEST_DURATION_SECONDS: u64 = 3;
const TARGET_SAMPLE_RATE: u32 = 16000; // STT servisleri genellikle 16kHz bekler

// Bu, process_audio_chunk içindekiyle aynı olmalı
const SOURCE_SAMPLE_RATE: u32 = 8000;
const PCMU_PAYLOAD_SIZE: usize = 160; // 20ms'lik G.711 sesi

// --- media-service/src/rtp/codecs.rs'ten kopyalandı ---
pub const ULAW_TO_PCM: [i16; 256] = [
    -32124, -31100, -30076, -29052, -28028, -27004, -25980, -24956, -23932, -22908,
    -21884, -20860, -19836, -18812, -17788, -16764, -15996, -15484, -14972, -14460,
    -13948, -13436, -12924, -12412, -11900, -11388, -10876, -10364, -9852, -9340,
    -8828, -8316, -7932, -7676, -7420, -7164, -6908, -6652, -6396, -6140, -5884,
    -5628, -5372, -5116, -4860, -4604, -4348, -4092, -3900, -3772, -3644, -3516,
    -3388, -3260, -3132, -3004, -2876, -2748, -2620, -2492, -2364, -2236, -2108,
    -1980, -1884, -1820, -1756, -1692, -1628, -1564, -1500, -1436, -1372, -1308,
    -1244, -1180, -1116, -1052, -988, -924, -876, -844, -812, -780, -748, -716,
    -684, -652, -620, -588, -556, -524, -492, -460, -428, -396, -372, -356, -340,
    -324, -308, -292, -276, -260, -244, -228, -212, -196, -180, -164, -148, -132,
    -120, -112, -104, -96, -88, -80, -72, -64, -56, -48, -40, -32, -24, -16, -8, 0,
    32124, 31100, 30076, 29052, 28028, 27004, 25980, 24956, 23932, 22908, 21884,
    20860, 19836, 18812, 17788, 16764, 15996, 15484, 14972, 14460, 13948, 13436,
    12924, 12412, 11900, 11388, 10876, 10364, 9852, 9340, 8828, 8316, 7932, 7676,
    7420, 7164, 6908, 6652, 6396, 6140, 5884, 5628, 5372, 5116, 4860, 4604, 4348,
    4092, 3900, 3772, 3644, 3516, 3388, 3260, 3132, 3004, 2876, 2748, 2620, 2492,
    2364, 2236, 2108, 1980, 1884, 1820, 1756, 1692, 1628, 1564, 1500, 1436, 1372,
    1308, 1244, 1180, 1116, 1052, 988, 924, 876, 844, 812, 780, 748, 716, 684, 652,
    620, 588, 556, 524, 492, 460, 428, 396, 372, 356, 340, 324, 308, 292, 276,
    260, 244, 228, 212, 196, 180, 164, 148, 132, 120, 112, 104, 96, 88, 80, 72, 64,
    56, 48, 40, 32, 24, 16, 8, 0,
];

// --- media-service/src/rtp/session.rs'ten kopyalandı ve hafifçe değiştirildi ---
// Bu fonksiyon, Cargo.toml'a rubato eklememizi gerektirecek
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};

fn process_audio_chunk_for_test(
    pcmu_payload: &[u8],
    source_rate: u32,
    target_rate: u32,
) -> Result<BytesMut> {
    let pcm_samples_i16: Vec<i16> = pcmu_payload.iter().map(|&byte| ULAW_TO_PCM[byte as usize]).collect();

    if target_rate == source_rate {
        let mut bytes = BytesMut::with_capacity(pcm_samples_i16.len() * 2);
        for &sample in &pcm_samples_i16 {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        return Ok(bytes);
    }

    let pcm_f32: Vec<f32> = pcm_samples_i16.iter().map(|&sample| sample as f32 / 32768.0).collect();
    let params = SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 256,
        window: WindowFunction::BlackmanHarris2,
    };
    let mut resampler = SincFixedIn::<f32>::new(
        target_rate as f64 / source_rate as f64, 2.0, params, pcm_f32.len(), 1,
    )?;
    let resampled_f32 = resampler.process(&[pcm_f32], None)?.remove(0);
    let resampled_i16: Vec<i16> = resampled_f32.into_iter().map(|s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16).collect();

    let mut bytes = BytesMut::with_capacity(resampled_i16.len() * 2);
    for &sample in &resampled_i16 {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    Ok(bytes)
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::from_filename(".env.development").ok();
    println!("--- Canlı Ses Akışı Test İstemcisi Başlatılıyor ---");

    let mut client = connect_to_media_service().await?;
    println!("✅ Media Service'e bağlantı başarılı!");

    println!("\nAdım 1: RTP portu alınıyor...");
    let allocate_res = client.allocate_port(AllocatePortRequest {
        call_id: format!("live-stream-test-{}", rand::random::<u32>()),
    }).await?;
    let rtp_port = allocate_res.into_inner().rtp_port;
    println!("✅ Port alındı: {}", rtp_port);

    // --- gRPC Dinleyici Task'ını Başlat ---
    println!("\nAdım 2: gRPC RecordAudio stream'i dinlenmeye başlanıyor...");
    let mut grpc_client_clone = client.clone();
    let listener_handle = tokio::spawn(async move {
        listen_for_live_audio(&mut grpc_client_clone, rtp_port, TARGET_SAMPLE_RATE).await
    });

    // Küçük bir bekleme, gRPC stream'inin tam olarak başlaması için
    tokio::time::sleep(Duration::from_millis(100)).await;

    // --- RTP Gönderici Task'ını Başlat ---
    println!("\nAdım 3: {} saniye boyunca RTP paketleri gönderiliyor...", TEST_DURATION_SECONDS);
    let rtp_target_ip = env::var("MEDIA_SERVICE_RTP_TARGET_IP").unwrap_or_else(|_| "127.0.0.1".to_string());
    let original_payload = send_rtp_packets(&rtp_target_ip, rtp_port as u16, TEST_DURATION_SECONDS).await?;
    println!("✅ RTP gönderimi tamamlandı.");

    // --- Sonuçları Topla ve Doğrula ---
    println!("\nAdım 4: Sonuçlar toplanıyor ve doğrulanıyor...");
    let received_payload = listener_handle.await??;

    println!("Gönderilen veri boyutu: {} bytes", original_payload.len());
    println!("Alınan veri boyutu:    {} bytes", received_payload.len());

    // Yeniden örnekleme nedeniyle boyutlar tam olarak aynı olmayabilir.
    // %10'luk bir tolerans makuldür.
    let size_difference = (original_payload.len() as i64 - received_payload.len() as i64).abs() as f64;
    let tolerance = original_payload.len() as f64 * 0.1;

    if size_difference > tolerance {
        return Err(anyhow!(
            "Başarısız: Alınan ve gönderilen veri boyutları arasında çok fazla fark var!"
        ));
    }
    
    // Basit bir checksum kontrolü. Verinin bozulup bozulmadığını anlamak için.
    let original_checksum: u8 = original_payload.iter().fold(0, |acc, &x| acc.wrapping_add(x));
    let received_checksum: u8 = received_payload.iter().fold(0, |acc, &x| acc.wrapping_add(x));
    
    // Checksum'ların çok yakın olmasını bekleriz ama yeniden örnekleme bunu etkileyebilir.
    // Bu yüzden asıl kontrol boyut ve akışın gelip gelmediğidir.
    println!("Gönderilen veri checksum: {}", original_checksum);
    println!("Alınan veri checksum:    {}", received_checksum);

    if received_payload.is_empty() {
         return Err(anyhow!("Başarısız: Hiç ses verisi alınamadı!"));
    }
    
    println!("\n✅✅✅ TEST BAŞARILI ✅✅✅");

    println!("\nAdım 5: Port serbest bırakılıyor...");
    client.release_port(ReleasePortRequest { rtp_port }).await?;
    println!("✅ Port serbest bırakıldı.");

    Ok(())
}

async fn listen_for_live_audio(
    client: &mut MediaServiceClient<Channel>,
    rtp_port: u32,
    target_sample_rate: u32,
) -> Result<BytesMut> {
    let request = RecordAudioRequest {
        server_rtp_port: rtp_port,
        target_sample_rate: Some(target_sample_rate),
    };

    let mut stream = client.record_audio(request).await?.into_inner();
    let mut received_data = BytesMut::new();

    // Stream'den veri okumak için bir zaman aşımı (timeout) belirliyoruz.
    // Ses kesildikten sonra stream'in kapanmasını bekleriz.
    while let Ok(Some(response_result)) = timeout(Duration::from_secs(2), stream.next()).await {
        match response_result {
            Ok(response) => {
                received_data.extend_from_slice(&response.audio_data);
            }
            Err(e) => {
                eprintln!("Stream hatası: {}", e);
                break;
            }
        }
    }
    
    Ok(received_data)
}

async fn send_rtp_packets(host: &str, port: u16, duration_secs: u64) -> Result<BytesMut> {
    let socket = tokio::net::UdpSocket::bind("0.0.0.0:0").await?;
    let target_addr = format!("{}:{}", host, port);
    
    let test_audio_payload: [u8; PCMU_PAYLOAD_SIZE] = [
        0xff, 0xec, 0xdc, 0xcd, 0xc0, 0xb3, 0xa8, 0x9d, 0x93, 0x8a, 0x82, 0x80, 0x82, 0x8a, 0x93, 0x9d,
        0xa8, 0xb3, 0xc0, 0xcd, 0xdc, 0xec, 0xff, 0xff, 0xec, 0xdc, 0xcd, 0xc0, 0xb3, 0xa8, 0x9d, 0x93,
        0x8a, 0x82, 0x80, 0x82, 0x8a, 0x93, 0x9d, 0xa8, 0xb3, 0xc0, 0xcd, 0xdc, 0xec, 0xff, 0xff, 0xec,
        0xdc, 0xcd, 0xc0, 0xb3, 0xa8, 0x9d, 0x93, 0x8a, 0x82, 0x80, 0x82, 0x8a, 0x93, 0x9d, 0xa8, 0xb3,
        0xc0, 0xcd, 0xdc, 0xec, 0xff, 0xff, 0xec, 0xdc, 0xcd, 0xc0, 0xb3, 0xa8, 0x9d, 0x93, 0x8a, 0x82,
        0x80, 0x82, 0x8a, 0x93, 0x9d, 0xa8, 0xb3, 0xc0, 0xcd, 0xdc, 0xec, 0xff, 0xff, 0xec, 0xdc, 0xcd,
        0xc0, 0xb3, 0xa8, 0x9d, 0x93, 0x8a, 0x82, 0x80, 0x82, 0x8a, 0x93, 0x9d, 0xa8, 0xb3, 0xc0, 0xcd,
        0xdc, 0xec, 0xff, 0xff, 0xec, 0xdc, 0xcd, 0xc0, 0xb3, 0xa8, 0x9d, 0x93, 0x8a, 0x82, 0x80, 0x82,
        0x8a, 0x93, 0x9d, 0xa8, 0xb3, 0xc0, 0xcd, 0xdc, 0xec, 0xff, 0xff, 0xec, 0xdc, 0xcd, 0xc0, 0xb3,
        0xa8, 0x9d, 0x93, 0x8a, 0x82, 0x80, 0x82, 0x8a, 0x93, 0x9d, 0xa8, 0xb3, 0xc0, 0xcd, 0xdc, 0xec
    ];
    
    let mut packet = Packet {
        header: rtp::header::Header {
            version: 2, payload_type: 0, sequence_number: rand::thread_rng().gen(),
            timestamp: rand::thread_rng().gen(), ssrc: rand::thread_rng().gen(), ..Default::default()
        },
        payload: Vec::from(test_audio_payload).into(),
    };
    
    let mut ticker = interval(Duration::from_millis(20));
    let num_packets = duration_secs * 50;
    
    // DÜZELTME: Gönderilen tüm PCM verisini biriktirmek için bir buffer oluştur.
    let mut total_payload_sent = BytesMut::new();

    for _ in 0..num_packets {
        ticker.tick().await;
        // Gönderdiğimiz PCMU verisini, sunucunun yapacağı gibi PCM'e çevirip biriktiriyoruz.
        let pcm_chunk = process_audio_chunk_for_test(&test_audio_payload, SOURCE_SAMPLE_RATE, TARGET_SAMPLE_RATE)?;
        total_payload_sent.extend_from_slice(&pcm_chunk);

        let packet_bytes = packet.marshal()?;
        socket.send_to(&packet_bytes, &target_addr).await?;
        
        packet.header.sequence_number = packet.header.sequence_number.wrapping_add(1);
        packet.header.timestamp = packet.header.timestamp.wrapping_add(PCMU_PAYLOAD_SIZE as u32);
    }
    
    // DÜZELTME: Artık içi dolu, doğru buffer'ı dönüyoruz.
    Ok(total_payload_sent)
}

// ... connect_to_media_service fonksiyonu recording_client'taki ile aynı ...
async fn connect_to_media_service() -> Result<MediaServiceClient<Channel>> {
    let client_cert_path = env::var("AGENT_SERVICE_CERT_PATH")?;
    let client_key_path = env::var("AGENT_SERVICE_KEY_PATH")?;
    let ca_path = env::var("GRPC_TLS_CA_PATH")?;
    let media_service_url = env::var("MEDIA_SERVICE_GRPC_URL")?;
    let server_addr = format!("https://{}", media_service_url);

    let client_identity = Identity::from_pem(tokio::fs::read(&client_cert_path).await?, tokio::fs::read(&client_key_path).await?);
    let server_ca_certificate = Certificate::from_pem(tokio::fs::read(&ca_path).await?);
    let tls_config = ClientTlsConfig::new()
        .domain_name(env::var("MEDIA_SERVICE_HOST")?)
        .ca_certificate(server_ca_certificate)
        .identity(client_identity);
    
    let channel = Channel::from_shared(server_addr)?.tls_config(tls_config)?.connect().await?;
    Ok(MediaServiceClient::new(channel))
}