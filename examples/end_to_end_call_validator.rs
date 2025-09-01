// File: examples/end_to_end_call_validator.rs

use anyhow::{Result, Context};
use aws_config::BehaviorVersion;
use aws_sdk_s3::Client as S3Client;
use bytes::Bytes;
use hound::WavReader;
use rand::Rng;
use rtp::packet::Packet;
use sentiric_contracts::sentiric::media::v1::{
    media_service_client::MediaServiceClient, AllocatePortRequest, PlayAudioRequest,
    RecordAudioRequest, ReleasePortRequest, StartRecordingRequest, StopRecordingRequest,
};
use std::env;
use std::f32::consts::PI;
use std::io::Cursor;
use std::net::UdpSocket;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::spawn_blocking;
use tokio::time::{sleep, timeout};
use tokio_stream::StreamExt;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};
use webrtc_util::marshal::Marshal;

fn linear_to_alaw(mut pcm_val: i16) -> u8 {
    let sign = (pcm_val >> 8) & 0x80; if sign != 0 { pcm_val = -pcm_val; }
    if pcm_val > 32635 { pcm_val = 32635; }
    let mut exponent: i16;
    if pcm_val >= 256 {
        exponent = 4; while exponent < 8 { if pcm_val < (256 << exponent) { break; } exponent += 1; }
        exponent -= 1;
    } else { exponent = (pcm_val >> 4) & 0x0F; }
    let mantissa = (pcm_val >> (if exponent > 1 { exponent } else { 1 })) & 0x0F;
    let alaw = (exponent << 4) | mantissa; (alaw ^ 0x55) as u8
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("--- 🎙️ Uçtan Uca Medya Servisi Doğrulama Testi Başlatılıyor (Docker Test Ortamı) ---");
    println!("---  Senaryo: PCMA kodek ile çağrı, 16kHz WAV olarak kayıt ve birleştirme ---");

    let mut client = connect_to_media_service().await?;
    let s3_client = connect_to_s3().await?;

    println!("\n[ADIM 1] Port alınıyor ve PCMA için kayıt başlatılıyor...");
    let allocate_res = client.allocate_port(AllocatePortRequest {
        call_id: format!("validation-call-{}", rand::random::<u32>()),
    }).await?.into_inner();
    let rtp_port = allocate_res.rtp_port;

    let s3_bucket = env::var("S3_BUCKET_NAME")?;
    let s3_key = format!("test/test_validation_{}.wav", rtp_port);
    let output_uri = format!("s3:///{}", s3_key);

    client.start_recording(StartRecordingRequest {
        server_rtp_port: rtp_port, output_uri: output_uri.clone(),
        sample_rate: None, format: None,
    }).await?;
    println!("✅ Kayıt başlatıldı. Hedef: s3://{}/{}", s3_bucket, s3_key);

    println!("\n[ADIM 2] Eş zamanlı medya akışları simüle ediliyor...");
    
    let rtp_target_ip = env::var("MEDIA_SERVICE_PUBLIC_IP")
        .context("MEDIA_SERVICE_PUBLIC_IP .env dosyasında eksik veya yanlış.")?;
    
    let bind_addr = "0.0.0.0:0";
    let local_rtp_socket = UdpSocket::bind(&bind_addr).context(format!("{} adresine bind edilemedi", bind_addr))?;
    let local_rtp_addr = local_rtp_socket.local_addr()?;
    println!("[İSTEMCİ] RTP anonsları şu adrese beklenecek: {}", local_rtp_addr);
    
    let (tx, mut rx) = mpsc::channel::<()>(1);

    let mut stt_client = client.clone();
    let stt_sim_handle = tokio::spawn(async move {
        listen_to_live_audio(&mut stt_client, rtp_port, &mut rx).await
    });
    
    let user_sim_handle = tokio::spawn(
        send_pcma_rtp_stream_blocking(rtp_target_ip.clone(), rtp_port as u16, Duration::from_secs(3), tx)
    );
    
    sleep(Duration::from_millis(500)).await;
    println!("[BOT SİM] 'welcome.wav' anonsu çalınıyor (PCMA olarak gönderilecek)...");
    
    client.play_audio(PlayAudioRequest {
        audio_uri: "file:///audio/tr/welcome.wav".to_string(),
        server_rtp_port: rtp_port,
        rtp_target_addr: local_rtp_addr.to_string(),
    }).await?;
    println!("[BOT SİM] Anons çalma komutu sunucuya başarıyla gönderildi (non-blocking).");
    
    user_sim_handle.await??;
    let received_audio_len = stt_sim_handle.await??;

    println!("✅ [STT SİM] {} byte temiz 16kHz LPCM ses verisi (sadece inbound) alındı.", received_audio_len);

    let expected_min_bytes = 80000;
    assert!(
        received_audio_len > expected_min_bytes, 
        "STT servisi yeterli ses verisi alamadı! (Beklenen > {}, Alınan: {})", 
        expected_min_bytes, received_audio_len
    );

    println!("\n[ADIM 3] Kayıt durduruluyor ve kaynaklar serbest bırakılıyor...");
    client.stop_recording(StopRecordingRequest { server_rtp_port: rtp_port }).await?;
    client.release_port(ReleasePortRequest { rtp_port }).await?;

    println!("\n[ADIM 4] Kayıt dosyası S3'ten indirilip doğrulanıyor...");
    let wav_data = download_from_s3(&s3_client, &s3_bucket, &s3_key).await?;
    println!("✅ Kayıt S3'ten indirildi ({} byte).", wav_data.len());
    
    let reader = WavReader::new(Cursor::new(wav_data))?;
    let spec = reader.spec();
    let duration = reader.duration() as f32 / spec.sample_rate as f32;

    println!("\n--- WAV Dosyası Analizi ---");
    println!("  - Süre: {:.2} saniye", duration);
    println!("  - Örnekleme Oranı: {} Hz", spec.sample_rate);
    println!("  - Bit Derinliği: {}", spec.bits_per_sample);
    println!("  - Kanal Sayısı: {}", spec.channels);

    assert_eq!(spec.sample_rate, 16000, "HATA: Kayıt örnekleme oranı 16kHz olmalı!");
    assert_eq!(spec.bits_per_sample, 16, "HATA: Kayıt bit derinliği 16-bit olmalı!");
    assert_eq!(spec.channels, 1, "HATA: Kayıt mono olmalı!");
    assert!(duration > 2.5, "HATA: Kayıt süresi çok kısa, muhtemelen sesler birleştirilmedi!");

    println!("\n\n✅✅✅ DOĞRULAMA BAŞARILI ✅✅✅");
    println!("Media Service, PCMA <-> 16kHz LPCM <-> WAV dönüşümünü, ses birleştirmeyi ve standart kaydı başarıyla tamamladı.");

    Ok(())
}

async fn send_pcma_rtp_stream_blocking(host: String, port: u16, duration: Duration, done_tx: mpsc::Sender<()>) -> Result<()> {
    spawn_blocking(move || {
        send_pcma_rtp_stream_sync(host, port, duration)
    }).await??;

    let _ = done_tx.send(()).await;
    Ok(())
}

fn send_pcma_rtp_stream_sync(host: String, port: u16, duration: Duration) -> Result<()> {
    let mut pcm_8k = Vec::new();
    let num_samples = (8000.0 * duration.as_secs_f32()) as usize;
    for i in 0..num_samples {
        let val = ((i as f32 * 440.0 * 2.0 * PI / 8000.0).sin() * 16384.0) as i16;
        pcm_8k.push(val);
    }
    let pcma_payload: Vec<u8> = pcm_8k.iter().map(|&s| linear_to_alaw(s)).collect();
    
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    let target_addr = format!("{}:{}", host, port);
    println!("[KULLANICI SİM] 3 saniye boyunca PCMA RTP akışı gönderiliyor -> {}", target_addr);

    let mut packet = Packet {
        header: rtp::header::Header { 
            version: 2, payload_type: 8, sequence_number: rand::thread_rng().gen(), 
            timestamp: rand::thread_rng().gen(), ssrc: rand::thread_rng().gen(), ..Default::default() 
        },
        payload: vec![].into(),
    };
    for chunk in pcma_payload.chunks(160) {
        packet.payload = Bytes::copy_from_slice(chunk);
        let raw_packet = packet.marshal()?;
        socket.send_to(&raw_packet, &target_addr)?;
        packet.header.sequence_number = packet.header.sequence_number.wrapping_add(1);
        packet.header.timestamp = packet.header.timestamp.wrapping_add(160);
        std::thread::sleep(Duration::from_millis(20));
    }
    println!("[KULLANICI SİM] PCMA gönderimi tamamlandı.");
    Ok(())
}

async fn listen_to_live_audio(client: &mut MediaServiceClient<Channel>, port: u32, done_rx: &mut mpsc::Receiver<()>) -> Result<usize> {
    let mut stream = client.record_audio(RecordAudioRequest {
        server_rtp_port: port, target_sample_rate: Some(16000),
    }).await?.into_inner();
    
    let mut total_bytes = 0;
    let mut done_signal_received = false;

    loop {
        tokio::select! {
            _ = done_rx.recv(), if !done_signal_received => {
                println!("[STT SİM] Kullanıcı konuşmasının bittiği sinyali alındı. Stream'in doğal olarak kapanması bekleniyor...");
                done_signal_received = true;
            },
            maybe_item = stream.next() => {
                match maybe_item {
                    Some(Ok(res)) => {
                        total_bytes += res.audio_data.len();
                    },
                    Some(Err(e)) => { 
                        eprintln!("[STT SİM] gRPC stream hatası: {}", e); 
                        break;
                    },
                    None => { 
                        println!("[STT SİM] Stream sunucu tarafından doğal olarak kapatıldı."); 
                        break;
                    }
                }
            }
        }
    }
    Ok(total_bytes)
}

async fn download_from_s3(client: &S3Client, bucket: &str, key: &str) -> Result<Vec<u8>> {
    let resp = client.get_object().bucket(bucket).key(key).send().await?;
    let data = resp.body.collect().await?.into_bytes().to_vec();
    Ok(data)
}

async fn connect_to_media_service() -> Result<MediaServiceClient<Channel>> {
    let client_cert_path = env::var("AGENT_SERVICE_CERT_PATH").context("AGENT_SERVICE_CERT_PATH eksik")?;
    let client_key_path = env::var("AGENT_SERVICE_KEY_PATH").context("AGENT_SERVICE_KEY_PATH eksik")?;
    let ca_path = env::var("GRPC_TLS_CA_PATH").context("GRPC_TLS_CA_PATH eksik")?;
    let media_service_url = env::var("MEDIA_SERVICE_GRPC_URL").context("MEDIA_SERVICE_GRPC_URL eksik")?;
    let server_addr = format!("https://{}", media_service_url);
    let client_identity = Identity::from_pem(tokio::fs::read(&client_cert_path).await?, tokio::fs::read(&client_key_path).await?);
    let server_ca_certificate = Certificate::from_pem(tokio::fs::read(&ca_path).await?);
    let tls_config = ClientTlsConfig::new()
        .domain_name(env::var("MEDIA_SERVICE_HOST").context("MEDIA_SERVICE_HOST eksik")?)
        .ca_certificate(server_ca_certificate).identity(client_identity);
    let channel = Channel::from_shared(server_addr)?.tls_config(tls_config)?.connect().await?;
    Ok(MediaServiceClient::new(channel))
}

async fn connect_to_s3() -> Result<S3Client> {
    env::var("S3_ACCESS_KEY_ID").context("S3_ACCESS_KEY_ID .env dosyasında eksik")?;
    env::var("S3_SECRET_ACCESS_KEY").context("S3_SECRET_ACCESS_KEY .env dosyasında eksik")?;
    let config = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let s3_config = aws_sdk_s3::config::Builder::from(&config)
        .endpoint_url(env::var("S3_ENDPOINT_URL").context("S3_ENDPOINT_URL eksik")?)
        .force_path_style(true)
        .region(aws_sdk_s3::config::Region::new(env::var("S3_REGION").context("S3_REGION eksik")?))
        .build();
    Ok(S3Client::from_conf(s3_config))
}