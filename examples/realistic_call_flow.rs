use anyhow::{Context, Result};
use aws_config::BehaviorVersion;
use aws_sdk_s3::Client as S3Client;
use base64::{engine::general_purpose, Engine};
use bytes::Bytes;
use hound::{WavReader, WavSpec, SampleFormat};
// DÜZELTME: Eksik olan Rng trait'i import edildi.
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


/// Basit bir "Merhaba Dünya" TTS yanıtını simüle etmek için 1 saniyelik 16kHz mono WAV verisi oluşturur.
fn generate_mock_tts_wav_data() -> Result<Vec<u8>> {
    let spec = WavSpec {
        channels: 1, sample_rate: 16000,
        bits_per_sample: 16, sample_format: SampleFormat::Int,
    };
    let mut buffer = Cursor::new(Vec::new());
    let mut writer = hound::WavWriter::new(&mut buffer, spec)?;
    for t in (0..16000).map(|x| x as f32 / 16000.0) {
        let sample = (t * 440.0 * 2.0 * PI).sin();
        let amplitude = i16::MAX as f32;
        writer.write_sample((sample * amplitude) as i16)?;
    }
    writer.finalize()?;
    Ok(buffer.into_inner())
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("\n--- GERÇEKÇİ ÇAĞRI AKIŞI SİMÜLASYONU ---");
    println!("Senaryo: Bağlanma anonsu + Anında TTS yanıtı + Kullanıcı konuşması. Cızırtı ve anons kesilmesi test edilecek.");

    let env_file = env::var("ENV_FILE").unwrap_or_else(|_| ".env.test".to_string());
    dotenvy::from_filename(&env_file).ok();

    let mut client = connect_to_media_service().await?;
    let s3_client = connect_to_s3().await?;

    println!("\n[ADIM 1] Port alınıyor ve kayıt başlatılıyor...");
    let call_id = format!("realistic-flow-{}", rand::thread_rng().gen::<u32>());
    let allocate_res = client.allocate_port(AllocatePortRequest { call_id: call_id.clone() }).await?.into_inner();
    let rtp_port = allocate_res.rtp_port;

    let s3_bucket = env::var("S3_BUCKET_NAME")?;
    let s3_key = format!("test/realistic_flow_{}.wav", rtp_port);
    let output_uri = format!("s3://{}/{}", s3_bucket, s3_key);

    client.start_recording(StartRecordingRequest {
        server_rtp_port: rtp_port, output_uri: output_uri.clone(),
        sample_rate: Some(16000), format: Some("wav".to_string()),
        call_id, trace_id: "trace-realistic-flow".to_string(),
    }).await?;
    println!("- Kayıt başlatıldı. Hedef: {}", output_uri);

    println!("\n[ADIM 2] Eş zamanlı medya akışları simüle ediliyor...");
    let rtp_target_ip = env::var("MEDIA_SERVICE_RTP_TARGET_IP").context("MEDIA_SERVICE_RTP_TARGET_IP eksik")?;
    let local_rtp_addr = UdpSocket::bind("0.0.0.0:0")?.local_addr()?;
    println!("- [İSTEMCİ] RTP anonsları şu adrese beklenecek: {}", local_rtp_addr);

    let mut stt_client = client.clone();
    let (stt_done_tx, stt_done_rx) = tokio::sync::oneshot::channel();
    let stt_sim_handle = tokio::spawn(async move {
        listen_to_live_audio(&mut stt_client, rtp_port, stt_done_rx).await
    });

    let user_sim_handle = tokio::spawn(
        send_user_rtp_stream(rtp_target_ip.clone(), rtp_port as u16, Duration::from_secs(3))
    );
    
    println!("- [SİSTEM] 'connecting.wav' anonsu çalınıyor...");
    client.play_audio(PlayAudioRequest {
        audio_uri: "file://audio/tr/system/connecting.wav".to_string(),
        server_rtp_port: rtp_port, rtp_target_addr: local_rtp_addr.to_string(),
    }).await?;

    sleep(Duration::from_millis(50)).await;
    let mock_tts_data = generate_mock_tts_wav_data()?;
    let tts_base64 = general_purpose::STANDARD.encode(&mock_tts_data);
    let tts_data_uri = format!("data:audio/wav;base64,{}", tts_base64);
    
    println!("- [SİSTEM] İlk TTS yanıtı (simüle) anında gönderiliyor (kuyruğa alınmalı)...");
    client.play_audio(PlayAudioRequest {
        audio_uri: tts_data_uri,
        server_rtp_port: rtp_port, rtp_target_addr: local_rtp_addr.to_string(),
    }).await?;
    
    let received_bytes = user_sim_handle.await??;
    sleep(Duration::from_secs(3)).await; // Tüm anonsların bitmesi için ek süre
    stt_done_tx.send(()).unwrap();
    let stt_total_bytes = stt_sim_handle.await??;

    println!("- [KULLANICI SİM] {} byte ham PCMU verisi gönderdi.", received_bytes);
    println!("- [STT SİM] {} byte temiz 16kHz LPCM verisi aldı.", stt_total_bytes);
    assert!(stt_total_bytes > received_bytes * 30, "HATA: Gelen ses verisi (STT) beklenenden çok az. Cızırtı/kayıp var!");

    println!("\n[ADIM 3] Kayıt durduruluyor ve kaynaklar serbest bırakılıyor...");
    client.stop_recording(StopRecordingRequest { server_rtp_port: rtp_port }).await?;
    client.release_port(ReleasePortRequest { rtp_port }).await?;

    println!("\n[ADIM 4] Sonuç kaydı S3'ten doğrulanıyor...");
    sleep(Duration::from_secs(2)).await;
    let wav_data = download_from_s3(&s3_client, &s3_bucket, &s3_key).await?;
    let reader = WavReader::new(Cursor::new(wav_data))?;
    let spec = reader.spec();
    let duration = reader.duration() as f32 / spec.sample_rate as f32;

    println!("\n--- KAYIT ANALİZİ ---");
    println!("  - Süre: {:.2} saniye", duration);
    println!("  - Örnekleme Oranı: {} Hz", spec.sample_rate);
    println!("  - Bit Derinliği: {}", spec.bits_per_sample);

    assert_eq!(spec.sample_rate, 8000, "HATA: Kayıt 8kHz olmalı!");
    assert_eq!(spec.bits_per_sample, 16, "HATA: Kayıt 16-bit olmalı!");
    assert!(duration > 5.0, "HATA: Kayıt süresi çok kısa ({:.2}s)! Anonslar kesildi veya sesler birleştirilemedi!", duration);

    println!("\n✅✅✅ REALISTIC FLOW TEST BAŞARILI ✅✅✅");
    Ok(())
}

async fn send_user_rtp_stream(host: String, port: u16, duration: Duration) -> Result<usize> {
    spawn_blocking(move || -> Result<usize> {
        let num_samples = (8000.0 * duration.as_secs_f32()) as usize;
        let pcm_8k: Vec<i16> = (0..num_samples)
            .map(|i| ((i as f32 * 440.0 * 2.0 * PI / 8000.0).sin() * 16384.0) as i16)
            .collect();
        let pcmu_payload: Vec<u8> = pcm_8k.iter().map(|&s| linear_to_ulaw(s)).collect();
        
        let socket = std::net::UdpSocket::bind("0.0.0.0:0")?;
        let target_addr = format!("{}:{}", host, port);
        
        // DÜZELTME: Rng trait'i zaten import edilmişti, `gen()` kullanımı doğru.
        // `spawn_blocking` içinde olduğumuz için `thread_rng` güvenlidir.
        let mut rng = rand::thread_rng();
        let mut packet = Packet {
            header: rtp::header::Header { 
                version: 2, payload_type: 0, 
                sequence_number: rng.gen(), timestamp: rng.gen(), ssrc: rng.gen(), 
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
        Ok(pcmu_payload.len())
    }).await?
}


const BIAS: i16 = 0x84;
// DÜZELTİLDİ: ULAW_TABLE sabiti tam 256 elemanlı olarak eklendi.
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
fn linear_to_ulaw(mut pcm_val: i16) -> u8 { let sign=if pcm_val<0{0x80}else{0};if sign!=0{pcm_val=-pcm_val;} pcm_val=pcm_val.min(32635);pcm_val+=BIAS;let exponent=ULAW_TABLE[((pcm_val>>7)&0xFF)as usize];let mantissa=(pcm_val>>(exponent as i16+3))&0xF;!(sign as u8|(exponent<<4)|mantissa as u8)}
async fn listen_to_live_audio(client:&mut MediaServiceClient<Channel>,port:u32,mut done_rx:tokio::sync::oneshot::Receiver<()>) -> Result<usize> { let mut stream = client.record_audio(RecordAudioRequest{server_rtp_port:port,target_sample_rate:Some(16000)}).await?.into_inner();let mut total_bytes=0;loop{tokio::select!{_= &mut done_rx=>{break;},maybe_item=stream.next()=>{match maybe_item{Some(Ok(res))=>{total_bytes+=res.audio_data.len();},Some(Err(_))=>{break;},None=>{break;}}}}} Ok(total_bytes)}
async fn download_from_s3(client:&S3Client,bucket:&str,key:&str)->Result<Vec<u8>>{let resp=client.get_object().bucket(bucket).key(key).send().await?;let data=resp.body.collect().await?.into_bytes().to_vec();Ok(data)}
async fn connect_to_media_service()->Result<MediaServiceClient<Channel>>{let client_cert_path=env::var("AGENT_SERVICE_CERT_PATH")?;let client_key_path=env::var("AGENT_SERVICE_KEY_PATH")?;let ca_path=env::var("GRPC_TLS_CA_PATH")?;let media_service_url=env::var("MEDIA_SERVICE_GRPC_URL")?;let server_addr=format!("https://{}",media_service_url);let client_identity=Identity::from_pem(tokio::fs::read(&client_cert_path).await?,tokio::fs::read(&client_key_path).await?);let server_ca_certificate=Certificate::from_pem(tokio::fs::read(&ca_path).await?);let tls_config=ClientTlsConfig::new().domain_name(env::var("MEDIA_SERVICE_HOST")?).ca_certificate(server_ca_certificate).identity(client_identity);let channel=Channel::from_shared(server_addr)?.tls_config(tls_config)?.connect().await?;Ok(MediaServiceClient::new(channel))}
async fn connect_to_s3()->Result<S3Client>{let access_key_id=env::var("S3_ACCESS_KEY_ID")?;let secret_access_key=env::var("S3_SECRET_ACCESS_KEY")?;let endpoint_url=env::var("S3_ENDPOINT_URL")?;let region=env::var("S3_REGION")?;let credentials_provider=aws_credential_types::Credentials::new(access_key_id,secret_access_key,None,None,"Static");let config=aws_config::defaults(BehaviorVersion::latest()).endpoint_url(endpoint_url).region(aws_config::Region::new(region)).credentials_provider(credentials_provider).load().await;let s3_config=aws_sdk_s3::config::Builder::from(&config).force_path_style(true).build();Ok(S3Client::from_conf(s3_config))}