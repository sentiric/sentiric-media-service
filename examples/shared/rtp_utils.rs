use anyhow::Result;
use bytes::Bytes;
use rand::Rng;
use rtp::packet::Packet;
use std::f32::consts::PI;
use std::time::Duration;
use tokio::net::UdpSocket;
use webrtc_util::marshal::Marshal;

// TEK VE DOĞRU ULAW_TABLE BURADA
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

pub async fn send_pcmu_rtp_stream(host: String, port: u16, duration: Duration, frequency: f32) -> Result<()> {
    let mut pcm_8k = Vec::new();
    let num_samples = (8000.0 * duration.as_secs_f32()) as usize;
    for i in 0..num_samples {
        let val = ((i as f32 * frequency * 2.0 * PI / 8000.0).sin() * 16384.0) as i16;
        pcm_8k.push(val);
    }
    let pcmu_payload: Vec<u8> = pcm_8k.iter().map(|&s| linear_to_ulaw(s)).collect();
    
    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    let target_addr = format!("{}:{}", host, port);
    println!("- [KULLANICI SIM] {}s boyunca {}Hz tonunda PCMU RTP akışı gönderiliyor -> {}", duration.as_secs(), frequency, target_addr);

    let (sequence_number, timestamp, ssrc) = {
        let mut rng = rand::thread_rng();
        (rng.gen(), rng.gen(), rng.gen())
    };

    let mut packet = Packet {
        header: rtp::header::Header { 
            version: 2, payload_type: 0, 
            sequence_number, timestamp, ssrc, 
            ..Default::default() 
        },
        payload: vec![].into(),
    };

    let mut ticker = tokio::time::interval(Duration::from_millis(20));
    for chunk in pcmu_payload.chunks(160) {
        ticker.tick().await;
        packet.payload = Bytes::copy_from_slice(chunk);
        let raw_packet = packet.marshal()?;
        socket.send_to(&raw_packet, &target_addr).await?;
        packet.header.sequence_number = packet.header.sequence_number.wrapping_add(1);
        packet.header.timestamp = packet.header.timestamp.wrapping_add(160);
    }
    println!("- [KULLANICI SIM] PCMU gönderimi tamamlandı.");
    Ok(())
}