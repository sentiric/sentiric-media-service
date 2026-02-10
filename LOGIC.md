# 🎙️ Media Service - Gerçek Zamanlı Medya Motoru

**Rol:** Ağdan gelen RTP paketlerini AI'ın anlayacağı dile (LPCM 16k) çeviren ve tam tersini yapan dönüştürücü.

## 1. Temel Sorumluluklar

1.  **Port Yönetimi:** `AllocatePort` ile UDP soketi açar, `ReleasePort` ile kapatır.
2.  **Akış İşleme:**
    *   Gelen paketleri `rtp-core` kullanarak decode eder.
    *   DTMF paketlerini (Payload 101) tanır ama ses olarak işlemez (Yoksayar).
    *   TTS'ten gelen sesi `rtp-core` kullanarak encode eder ve gönderir.
3.  **Kayıt:** Sesleri birleştirip S3'e yazar.

## 2. Bağımlılıklar

*   **`rtp-core`:** Tüm kodek, DSP ve paketleme mantığı buradan gelir. Media Service matematik yapmaz, kütüphaneyi çağırır.