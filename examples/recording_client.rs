use anyhow::Result;
use std::env;
use std::net::UdpSocket;
use std::time::Duration;
use webrtc_util::marshal::Marshal;

use sentiric_contracts::sentiric::media::v1::{
    media_service_client::MediaServiceClient,
    AllocatePortRequest, ReleasePortRequest, StartRecordingRequest, StopRecordingRequest
};
use tokio::time::sleep;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};
use rtp::packet::Packet;
use rand::Rng;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::from_filename(".env.test").ok();
    println!("--- Gercek Kayit Simulasyonu (Programatik RTP Akisi ile) ---");

    let mut client = connect_to_media_service().await?;
    println!("- Media Service'e baglanti basarili!");

    let call_id = format!("real-rec-call-{}", rand::random::<u32>());
    let trace_id = format!("trace-{}", rand::random::<u32>());

    let allocate_res = client.allocate_port(AllocatePortRequest {
        call_id: call_id.clone(),
    }).await?;
    let rtp_port = allocate_res.into_inner().rtp_port;
    // MEDIA_SERVICE_RECORD_BASE_PATH="/sentiric-media-record"
    let output_uri = format!("s3:///sentiric-media-record/recording_client_{}.wav", rtp_port);

    
    println!("\nAdim 1: Kayit baslatiliyor. Hedef: {}", output_uri);
    client.start_recording(StartRecordingRequest {
        server_rtp_port: rtp_port,
        output_uri: output_uri.clone(),
        sample_rate: Some(8000),
        format: Some("wav".to_string()),
        call_id,
        trace_id,
    }).await?;
    println!("- Kayit baslatma komutu gonderildi.");

    println!("\nAdim 2: Ayri bir task uzerinden RTP ses akisi baslatiliyor...");

    let rtp_target_ip = env::var("MEDIA_SERVICE_RTP_TARGET_IP")
    .unwrap_or_else(|_| "127.0.0.1".to_string());

    let rtp_stream_handle = tokio::spawn(async move {
        send_test_rtp_stream(&rtp_target_ip, rtp_port as u16).await;
    });
    
    println!("(Ana task, ses akisinin tamamlanmasi icin 5 saniye bekliyor...)");
    sleep(Duration::from_secs(5)).await;

    rtp_stream_handle.await?;

    println!("\nAdim 3: Kayit durduruluyor...");
    client.stop_recording(StopRecordingRequest { server_rtp_port: rtp_port }).await?;
    println!("- Kayit durdurma komutu gonderildi.");

    sleep(Duration::from_secs(1)).await;

    println!("\nAdim 4: Port serbest birakiliyor...");
    client.release_port(ReleasePortRequest { rtp_port }).await?;
    println!("- Port serbest birakildi.");
    
    println!("\n--- Simulasyon Tamamlandi ---");
    println!("Kayit dosyasi MinIO bucket'inda ({}) olusturulmus olmali.", output_uri);
    Ok(())
}

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
    
    println!("Media Service'e baglaniliyor: {}", server_addr);
    let channel = Channel::from_shared(server_addr)?.tls_config(tls_config)?.connect().await?;
    Ok(MediaServiceClient::new(channel))
}

async fn send_test_rtp_stream(host: &str, port: u16) {
    let test_audio_payload: [u8; 160] = [
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
    let target_addr = format!("{}:{}", host, port);
    println!("[RTP Gonderici] Hedef: {}", target_addr);
    let socket = UdpSocket::bind("0.0.0.0:0").unwrap();
    let mut packet = Packet { header: rtp::header::Header { version: 2, payload_type: 0, sequence_number: rand::thread_rng().gen(), timestamp: rand::thread_rng().gen(), ssrc: rand::thread_rng().gen(), ..Default::default() }, payload: Vec::from(test_audio_payload).into(), };
    println!("[RTP Gonderici] 2 saniye boyunca ses gonderiliyor...");
    for _ in 0..100 {
        let packet_bytes = packet.marshal().unwrap();
        if let Err(e) = socket.send_to(&packet_bytes, &target_addr) { eprintln!("[RTP Gonderici] Paket gonderilemedi: {}", e); break; }
        packet.header.sequence_number = packet.header.sequence_number.wrapping_add(1);
        packet.header.timestamp = packet.header.timestamp.wrapping_add(160);
        sleep(Duration::from_millis(20)).await;
    }
    println!("[RTP Gonderici] Ses gonderme tamamlandi.");
}