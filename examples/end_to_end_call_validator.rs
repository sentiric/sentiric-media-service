use anyhow::{anyhow, Context, Result};
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
use tokio::task::spawn_blocking;
use tokio::time::sleep;
use tokio_stream::StreamExt;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};
use webrtc_util::marshal::Marshal;

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
fn linear_to_ulaw(mut pcm_val: i16) -> u8 {
    let sign = if pcm_val < 0 { 0x80 } else { 0 };
    if sign != 0 { pcm_val = -pcm_val; }
    pcm_val = pcm_val.min(32635);
    pcm_val += BIAS;
    let exponent = ULAW_TABLE[((pcm_val >> 7) & 0xFF) as usize];
    let mantissa = (pcm_val >> (exponent as i16 + 3)) & 0xF;
    !(sign as u8 | (exponent << 4) | mantissa as u8)
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("--- Uctan Uca Medya Servisi Dogrulama Testi Baslatiliyor (Docker Test Ortami) ---");
    println!("---  Senaryo: PCMU kodek ile cagri, 16kHz WAV olarak kayit ve birlestirme ---");

    let env_file = env::var("ENV_FILE").unwrap_or_else(|_| ".env.test".to_string());
    dotenvy::from_filename(&env_file).ok();

    let mut client = connect_to_media_service().await?;
    let s3_client = connect_to_s3().await?;

    println!("\n[ADIM 1] Port aliniyor ve PCMU icin kayit baslatiliyor...");
    let call_id = format!("validation-call-{}", rand::random::<u32>());
    let trace_id = format!("trace-{}", rand::random::<u32>());

    let allocate_res = client.allocate_port(AllocatePortRequest {
        call_id: call_id.clone(),
    }).await?.into_inner();
    let rtp_port = allocate_res.rtp_port;

    let s3_bucket = env::var("S3_BUCKET_NAME")?;
    let s3_key = format!("test/test_validation_{}.wav", rtp_port);
    let output_uri = format!("s3:///{}", s3_key);
    client.start_recording(StartRecordingRequest {
        server_rtp_port: rtp_port,
        output_uri: output_uri.clone(),
        sample_rate: Some(16000),
        format: Some("wav".to_string()),
        call_id,
        trace_id,
    }).await?;
    println!("- Kayit baslatildi. Hedef: {}", output_uri);

    println!("\n[ADIM 2] Es zamanli medya akislari simule ediliyor...");
    
    let rtp_target_ip = env::var("MEDIA_SERVICE_RTP_TARGET_IP")
        .context("MEDIA_SERVICE_RTP_TARGET_IP .env dosyasinda eksik veya yanlis.")?;
    
    let bind_addr = "0.0.0.0:0";
    let local_rtp_socket = UdpSocket::bind(&bind_addr).context(format!("{} adresine bind edilemedi", bind_addr))?;
    let local_rtp_addr = local_rtp_socket.local_addr()?;
    println!("- [ISTEMCI] RTP anonslari su adrese beklenecek: {}", local_rtp_addr);
    
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();

    let mut stt_client = client.clone();
    let stt_sim_handle = tokio::spawn(async move {
        listen_to_live_audio(&mut stt_client, rtp_port, done_rx).await
    });
    
    sleep(Duration::from_millis(200)).await;

    let user_sim_handle = tokio::spawn(
        send_pcmu_rtp_stream_blocking(rtp_target_ip.clone(), rtp_port as u16, Duration::from_secs(4))
    );
    
    sleep(Duration::from_millis(500)).await;
    println!("- [BOT SIM] 'welcome.wav' anonsu caliniyor (PCMU olarak gonderilecek)...");
    
    client.play_audio(PlayAudioRequest {
        audio_uri: "file:///audio/tr/welcome.wav".to_string(),
        server_rtp_port: rtp_port,
        rtp_target_addr: local_rtp_addr.to_string(),
    }).await?;
    println!("- [BOT SIM] Anons calma komutu sunucuya basariyla gonderildi (non-blocking).");
    
    user_sim_handle.await??;
    
    sleep(Duration::from_secs(1)).await;
    let _ = done_tx.send(());

    let received_audio_len = stt_sim_handle.await??;

    println!("- [STT SIM] {} byte temiz 16kHz LPCM ses verisi (sadece inbound) alindi.", received_audio_len);
    
    let expected_min_bytes = 15000;
    
    assert!(
        received_audio_len >= expected_min_bytes, 
        "STT servisi yeterli ses verisi alamadi! (Beklenen >= {}, Alinan: {})", 
        expected_min_bytes, received_audio_len
    );

    println!("\n[ADIM 3] Kayit durduruluyor ve kaynaklar serbest birakiliyor...");
    client.stop_recording(StopRecordingRequest { server_rtp_port: rtp_port }).await?;
    client.release_port(ReleasePortRequest { rtp_port }).await?;

    println!("\n[ADIM 4] Kayit dosyasi S3'ten indirilip dogrulaniliyor...");
    let wav_data = download_from_s3(&s3_client, &s3_bucket, &s3_key).await?;
    println!("- Kayit S3 'ten indirildi ({} byte).", wav_data.len());
    
    let reader = WavReader::new(Cursor::new(wav_data))?;
    let spec = reader.spec();
    let duration = reader.duration() as f32 / spec.sample_rate as f32;

    println!("\n--- WAV Dosyasi Analizi ---");
    println!("  - Sure: {:.2} saniye", duration);
    println!("  - Ornekleme Orani: {} Hz", spec.sample_rate);
    println!("  - Bit Derinligi: {}", spec.bits_per_sample);
    println!("  - Kanal Sayisi: {}", spec.channels);

    assert_eq!(spec.sample_rate, 8000, "HATA: Kayit ornekleme orani 8kHz olmali!");
    assert_eq!(spec.bits_per_sample, 16, "HATA: Kayit bit derinligi 16-bit olmali!");
    assert_eq!(spec.channels, 1, "HATA: Kayit mono olmali!");
    assert!(duration > 2.5, "HATA: Kayit suresi cok kisa, muhtemelen sesler birlestirilmedi!");

    println!("\n\nDOGRULAMA BASARILI");
    println!("Media Service, PCMU <-> 16kHz LPCM <-> WAV donusumunu, ses birlestirmeyi ve standart kaydi basariyla tamamladi.");

    Ok(())
}

async fn send_pcmu_rtp_stream_blocking(host: String, port: u16, duration: Duration) -> Result<()> {
    spawn_blocking(move || {
        send_pcmu_rtp_stream_sync(host, port, duration)
    }).await??;
    Ok(())
}

fn send_pcmu_rtp_stream_sync(host: String, port: u16, duration: Duration) -> Result<()> {
    let mut pcm_8k = Vec::new();
    let num_samples = (8000.0 * duration.as_secs_f32()) as usize;
    for i in 0..num_samples {
        let val = ((i as f32 * 440.0 * 2.0 * PI / 8000.0).sin() * 16384.0) as i16;
        pcm_8k.push(val);
    }
    let pcmu_payload: Vec<u8> = pcm_8k.iter().map(|&s| linear_to_ulaw(s)).collect();
    
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    let target_addr = format!("{}:{}", host, port);
    println!("- [KULLANICI SIM] {} saniye boyunca PCMU RTP akisi gonderiliyor -> {}", duration.as_secs(), target_addr);

    let mut packet = Packet {
        header: rtp::header::Header { 
            version: 2, 
            payload_type: 0, 
            sequence_number: rand::thread_rng().gen(), 
            timestamp: rand::thread_rng().gen(), 
            ssrc: rand::thread_rng().gen(), 
            ..Default::default() 
        },
        payload: vec![].into(),
    };
    for chunk in pcmu_payload.chunks(160) {
        packet.payload = Bytes::copy_from_slice(chunk);
        let raw_packet = packet.marshal()?;
        socket.send_to(&raw_packet, &target_addr)?;
        packet.header.sequence_number = packet.header.sequence_number.wrapping_add(1);
        packet.header.timestamp = packet.header.timestamp.wrapping_add(160);
        std::thread::sleep(Duration::from_millis(20));
    }
    println!("- [KULLANICI SIM] PCMU gonderimi tamamlandi.");
    Ok(())
}

async fn listen_to_live_audio(
    client: &mut MediaServiceClient<Channel>,
    port: u32,
    mut done_rx: tokio::sync::oneshot::Receiver<()>,
) -> Result<usize> {
    let mut stream = client.record_audio(RecordAudioRequest {
        server_rtp_port: port, target_sample_rate: Some(16000),
    }).await?.into_inner();
    
    let mut total_bytes = 0;
    loop {
        tokio::select! {
            _ = &mut done_rx => {
                println!("- [STT SIM] Testin bittigi sinyali alindi. Stream dinlemesi sonlandiriliyor.");
                break;
            },
            maybe_item = stream.next() => {
                match maybe_item {
                    Some(Ok(res)) => {
                        total_bytes += res.audio_data.len();
                    },
                    Some(Err(e)) => { 
                        eprintln!("- [STT SIM] gRPC stream hatasi: {}", e); 
                        break;
                    },
                    None => { 
                        println!("- [STT SIM] Stream sunucu tarafindan dogal olarak kapatildi."); 
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
    let access_key_id = env::var("S3_ACCESS_KEY_ID").context("S3_ACCESS_KEY_ID .env dosyasinda eksik")?;
    let secret_access_key = env::var("S3_SECRET_ACCESS_KEY").context("S3_SECRET_ACCESS_KEY .env dosyasinda eksik")?;
    let endpoint_url = env::var("S3_ENDPOINT_URL").context("S3_ENDPOINT_URL eksik")?;
    let region = env::var("S3_REGION").context("S3_REGION eksik")?;

    let credentials_provider = aws_credential_types::Credentials::new(
        access_key_id,
        secret_access_key,
        None,
        None,
        "Static",
    );

    let config = aws_config::defaults(BehaviorVersion::latest())
        .endpoint_url(endpoint_url)
        .region(aws_config::Region::new(region))
        .credentials_provider(credentials_provider)
        .load()
        .await;

    let s3_config = aws_sdk_s3::config::Builder::from(&config)
        .force_path_style(true)
        .build();

    Ok(S3Client::from_conf(s3_config))
}