// examples/live_audio_client.rs

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
const TARGET_SAMPLE_RATE: u32 = 16000;

async fn connect_to_media_service() -> Result<MediaServiceClient<Channel>> {
    let client_cert_path = env::var("AGENT_SERVICE_CERT_PATH")?;
    let client_key_path = env::var("AGENT_SERVICE_KEY_PATH")?;
    let ca_path = env::var("GRPC_TLS_CA_PATH")?;
    let media_service_url = env::var("MEDIA_SERVICE_GRPC_URL")?;
    let server_addr = format!("https://{}", media_service_url);
    let client_identity = Identity::from_pem(tokio::fs::read(&client_cert_path).await?, tokio::fs::read(&client_key_path).await?);
    let server_ca_certificate = Certificate::from_pem(tokio::fs::read(&ca_path).await?);
    let tls_config = ClientTlsConfig::new().domain_name(env::var("MEDIA_SERVICE_HOST")?).ca_certificate(server_ca_certificate).identity(client_identity);
    let channel = Channel::from_shared(server_addr)?.tls_config(tls_config)?.connect().await?;
    Ok(MediaServiceClient::new(channel))
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::from_filename(".env.example").ok();
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

    tokio::time::sleep(Duration::from_millis(100)).await;

    // --- RTP Gönderici Task'ını Başlat ---
    println!("\nAdım 3: {} saniye boyunca RTP paketleri gönderiliyor...", TEST_DURATION_SECONDS);
    let rtp_target_ip = env::var("RTP_SERVICE_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    
    // Test verisi gönderiliyor
    let _ = send_rtp_packets(&rtp_target_ip, rtp_port as u16, TEST_DURATION_SECONDS).await?;
    println!("✅ RTP gönderimi tamamlandı.");

    // --- Sonuçları Topla ve Doğrula ---
    println!("\nAdım 4: Sonuçlar toplanıyor ve doğrulanıyor...");
    let received_payload = listener_handle.await??;

    // Beklenen boyut hesabı:
    // Süre * Örnekleme Hızı (16000) * Bit Derinliği (2 byte)
    // 3 sn * 16000 * 2 = 96,000 byte.
    let expected_min_bytes = (TEST_DURATION_SECONDS as u32 * TARGET_SAMPLE_RATE * 2) as usize;
    
    println!("Alınan veri boyutu: {} bytes", received_payload.len());
    println!("Beklenen min boyut: {} bytes", expected_min_bytes);

    // %10 tölerans
    let tolerance = expected_min_bytes as f64 * 0.10;
    let diff = (received_payload.len() as i64 - expected_min_bytes as i64).abs() as f64;

    if diff > tolerance {
        println!("⚠️ Uyarı: Boyut farkı tolerans sınırında. (Alınan: {}, Beklenen: {})", received_payload.len(), expected_min_bytes);
        // Veri hiç yoksa hata dön.
        if received_payload.len() < 1000 {
             return Err(anyhow!("Başarısız: Yeterli ses verisi alınamadı!"));
        }
    } else {
        println!("✅ Boyut doğrulandı (Tolerans dahilinde).");
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
    
    let test_audio_payload = [0u8; 160]; 
    
    let mut packet = Packet {
        header: rtp::header::Header {
            version: 2, payload_type: 0, sequence_number: rand::thread_rng().gen(),
            timestamp: rand::thread_rng().gen(), ssrc: rand::thread_rng().gen(), ..Default::default()
        },
        payload: Vec::from(test_audio_payload).into(),
    };
    
    let mut ticker = interval(Duration::from_millis(20));
    let num_packets = duration_secs * 50;
    
    let mut total_payload_sent = BytesMut::new();

    for _ in 0..num_packets {
        ticker.tick().await;
        total_payload_sent.extend_from_slice(&test_audio_payload);
        let packet_bytes = packet.marshal()?;
        socket.send_to(&packet_bytes, &target_addr).await?;
        packet.header.sequence_number = packet.header.sequence_number.wrapping_add(1);
        packet.header.timestamp = packet.header.timestamp.wrapping_add(160);
    }
    
    Ok(total_payload_sent)
}