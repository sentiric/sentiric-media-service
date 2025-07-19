const express = require('express');
const dgram = require('dgram');
const { spawn } = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');

const app = express();
app.use(express.json({ limit: '10mb' }));

const PORT = process.env.PORT || 3003;
const PUBLIC_IP = process.env.PUBLIC_IP || '127.0.0.1';
const RTP_PORT_MIN = parseInt(process.env.RTP_PORT_MIN) || 10000;
const RTP_PORT_MAX = parseInt(process.env.RTP_PORT_MAX) || 10100;
const usedPorts = new Set();
const rtpSockets = {}; // Aktif soketleri ve karşı taraf bilgilerini saklamak için

console.log(`[Media Service] RTP port aralığı: ${RTP_PORT_MIN}-${RTP_PORT_MAX}`);

const allocateRtpPort = () => {
  for (let i = 0; i < 100; i++) {
    const port = Math.floor(Math.random() * (RTP_PORT_MAX - RTP_PORT_MIN + 1)) + RTP_PORT_MIN;
    if (port % 2 === 0 && !usedPorts.has(port)) {
      usedPorts.add(port);
      console.log(`[Media Service] Yeni RTP portu ayrıldı: ${port}`);
      return port;
    }
  }
  return null;
};

app.get('/rtp-session', (req, res) => {
    console.log('[Media Service] Yeni RTP oturumu için istek alındı.');
    const port = allocateRtpPort();

    if (port) {
        const rtpSocket = dgram.createSocket('udp4');
        rtpSocket.on('message', (msg, rinfo) => {
            console.log(`[RTP:${port}] Medya paketi alındı from ${rinfo.address}:${rinfo.port} - Boyut: ${msg.length} bytes`);
            // Karşı tarafın adresini ilk paketten öğren ve sakla
            if (!rtpSockets[port] || !rtpSockets[port].remoteInfo) {
                 console.log(`[RTP:${port}] Karşı tarafın adresi öğrenildi: ${rinfo.address}:${rinfo.port}`);
                 rtpSockets[port].remoteInfo = rinfo;
            }
        });
        rtpSocket.bind(port, '0.0.0.0');
        rtpSockets[port] = { socket: rtpSocket, remoteInfo: null }; // Soketi ve remoteInfo'yu sakla

        res.status(200).json({ host: PUBLIC_IP, port: port });
    } else {
        console.error('--> ❌ Uygun RTP portu bulunamadı.');
        res.status(500).json({ error: 'Could not allocate RTP port' });
    }
});

app.post('/play-audio', (req, res) => {
    const { rtp_port, audio_data_base64, format } = req.body;
    const session = rtpSockets[rtp_port];

    console.log(`[Media Service] ${rtp_port} portundan ses çalma isteği alındı.`);

    if (!session || !audio_data_base64) {
        return res.status(400).send({ error: 'Geçersiz istek: rtp_port veya audio_data_base64 eksik.' });
    }

    const maxWaitTime = 5000; // Maksimum 5 saniye bekle
    const checkInterval = 500; // Her 500ms'de bir kontrol et
    let timeWaited = 0;

    const playWhenReady = () => {
        const targetInfo = session.remoteInfo;

        if (targetInfo) {
            // Hedef bulundu, FFmpeg'i başlat!
            const tmpDir = os.tmpdir();
            const inputFile = path.join(tmpDir, `audio_${rtp_port}.mp3`);
            fs.writeFileSync(inputFile, Buffer.from(audio_data_base64, 'base64'));

            console.log(`--> FFmpeg ile ${inputFile} dosyası ${targetInfo.address}:${targetInfo.port} adresine stream edilecek.`);
            
            const ffmpeg = spawn('ffmpeg', [
                '-re', '-i', inputFile, '-ar', '8000', '-ac', '1',
                '-acodec', 'pcm_mulaw', '-f', 'rtp',
                `rtp://${targetInfo.address}:${targetInfo.port}?pkt_size=160`
            ]);

            ffmpeg.stderr.on('data', (data) => console.log(`[FFmpeg stderr]: ${data.toString()}`));
            ffmpeg.on('close', (code) => {
                console.log(`--> FFmpeg işlemi tamamlandı. Kod: ${code}`);
                try { fs.unlinkSync(inputFile); } catch (e) {}
            });
        } else if (timeWaited < maxWaitTime) {
            // Henüz hedef yok, beklemeye devam et.
            console.warn(`[RTP:${rtp_port}] Henüz karşı taraf adresi bilinmiyor. ${checkInterval}ms sonra tekrar denenecek.`);
            timeWaited += checkInterval;
            setTimeout(playWhenReady, checkInterval);
        } else {
            // Zaman aşımına uğradı.
            console.error(`❌ [RTP:${rtp_port}] Zaman aşımı: ${maxWaitTime}ms içinde karşı taraftan RTP paketi alınamadı. Ses çalma iptal edildi.`);
        }
    };
    
    playWhenReady();
    res.status(202).send({ status: 'Ses çalma işlemi başlatıldı. Karşı taraftan ilk RTP paketi bekleniyor.' });
});

app.listen(PORT, '0.0.0.0', () => {
    console.log(`✅ [Media Service] Servis http://0.0.0.0:${PORT} adresinde dinlemede.`);
});