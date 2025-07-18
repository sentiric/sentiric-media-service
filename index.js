const express = require('express');
const dgram = require('dgram'); // RTP için UDP soketleri kullanacağız.

const app = express();
const PORT = process.env.PORT || 3003;

// RTP portları için kullanılacak aralığı ortam değişkenlerinden alıyoruz.
const RTP_PORT_MIN = parseInt(process.env.RTP_PORT_MIN, 10) || 10000;
const RTP_PORT_MAX = parseInt(process.env.RTP_PORT_MAX, 10) || 20000;
const usedPorts = new Set();

// Bu fonksiyon, belirtilen aralıkta rastgele ve kullanılmayan bir port bulur.
const allocateRtpPort = () => {
  for (let i = 0; i < 100; i++) { // Sonsuz döngüyü önlemek için 100 deneme hakkı.
    const port = Math.floor(Math.random() * (RTP_PORT_MAX - RTP_PORT_MIN + 1)) + RTP_PORT_MIN;
    // Port çift olmalı (RTP için tek, RTCP için bir sonraki çift port kullanılır).
    if (port % 2 === 0 && !usedPorts.has(port)) {
      usedPorts.add(port);
      console.log(`[Media Service] Yeni RTP portu ayrıldı: ${port}`);
      return port;
    }
  }
  return null; // Uygun port bulunamadı.
};

// Yeni bir medya oturumu (RTP portu) talep etmek için endpoint.
// Gerçek bir uygulamada bu POST olmalı, basitlik için GET kullanıyoruz.
app.get('/rtp-session', (req, res) => {
  console.log('[Media Service] Yeni RTP oturumu için istek alındı.');
  const port = allocateRtpPort();

  if (port) {
    // Portu dinleyecek basit bir UDP soketi oluşturuyoruz.
    // Bu, portun "canlı" olduğunu gösterir. Henüz gelen veriyi işlemiyoruz.
    const rtpSocket = dgram.createSocket('udp4');
    rtpSocket.on('message', (msg, rinfo) => {
        // Gelen RTP paketlerini şimdilik sadece log'luyoruz.
        console.log(`[RTP:${port}] Medya paketi alındı from ${rinfo.address}:${rinfo.port} - Boyut: ${msg.length} bytes`);
    });
    rtpSocket.bind(port, '0.0.0.0');

    res.status(200).json({
      host: process.env.PUBLIC_IP, // Sunucunun genel IP'si .env'den gelecek
      port: port
    });
  } else {
    console.error('--> ❌ Uygun RTP portu bulunamadı.');
    res.status(500).json({ error: 'Could not allocate RTP port' });
  }
});

app.listen(PORT, '0.0.0.0', () => {
  console.log(`✅ [Media Service] Servis http://0.0.0.0:${PORT} adresinde dinlemede.`);
});