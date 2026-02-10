// sentiric-media-service/src/rtp/stream.rs
// [ARCHITECTURAL REFACTOR]
// Bu dosyanın içeriği rtp/session.rs içindeki `send_raw_rtp` ve
// rtp/codecs.rs içindeki `encode_lpcm16_to_rtp` fonksiyonlarına
// entegre edilmiştir. Artık bu dosyaya ihtiyaç kalmamıştır.
// Temizlik ve merkeziyetçilik adına bu dosya silinebilir.
// Şimdilik boş bırakıyoruz.