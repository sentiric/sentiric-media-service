# 🎙️ Sentiric Media Service - Geliştirme Yol Haritası (v5.4 - Kayıt Stabilizasyonu)

Bu belge, `media-service`'in, `sentiric-governance` anayasasında tanımlanan rolünü eksiksiz bir şekilde yerine getirmesi için gereken tüm görevleri listeler.

---

### **FAZ 1: Stabilizasyon ve Temel Yetenekler (Tamamlanmış Görevler)**

-   [x] **Görev ID: MEDIA-REFACTOR-01 - Merkezi Ses İşleme ve Transcoding Motoru**
    -   **Durum:** ✅ **Tamamlandı**
    -   **Açıklama:** Gelen/giden tüm ses akışları standart 16kHz LPCM formatına dönüştürülüyor. **YENİ:** Kalıcı kayıtlar, son kullanıcı tarafından dinlenebilir olması için **8kHz mono WAV** formatına geri dönüştürülerek (downsampling) kaydediliyor. Bu, "hızlandırılmış ses" sorununu tamamen çözer.

-   [x] **Görev ID: MEDIA-001B - Kalıcı Çağrı Kaydı**
    -   **Durum:** ✅ **Tamamlandı**

-   [x] **Görev ID: DEVOPS-001 - Lokal S3 Simülasyon Ortamı**
    -   **Durum:** ✅ **Tamamlandı**

-   [x] **Görev ID: MEDIA-FEAT-03 - RabbitMQ Publisher Entegrasyonu**
    -   **Durum:** ✅ **Tamamlandı**

-   [x] **Görev ID: MEDIA-004 - Kayıt Tamamlandığında Olay Yayınlama**
    -   **Durum:** ✅ **Tamamlandı**
    -   **Açıklama:** `call.recording.available` olayı yayınlanırken, `RecordingSession` içinden alınan `call_id` ve `trace_id` bilgileri de JSON payload'una eklendi. Bu, `cdr-service`'in kaydı doğru çağrıyla ilişkilendirmesini sağlayarak veri bütünlüğünü garanti altına alır.

---