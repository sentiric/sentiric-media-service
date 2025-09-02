# 🎙️ Sentiric Media Service - Görev Listesi (v5.2 - Olay Odaklı Mimari)

Bu belge, media-service'in platformun geri kalanını bilgilendirme ve veri bütünlüğünü sağlama görevlerini tanımlar.

---

### **FAZ 1: Stabilizasyon (Tamamlanmış Görevler)**
*   [x] **MEDIA-REFACTOR-01**: Merkezi Ses İşleme ve Transcoding Motoru.
*   [x] **MEDIA-001B**: Kalıcı Çağrı Kaydı (S3 Desteği ile).
*   [x] **DEVOPS-001**: Lokal S3 Simülasyon Ortamı (MinIO).

---

### **FAZ 2: Platform Entegrasyonu ve Veri Akışı (Mevcut Odak)**

-   **Görev ID: MEDIA-FEAT-03 - RabbitMQ Publisher Entegrasyonu**
    -   **Durum:** ⬜ **Yapılacak**
    -   **Öncelik:** **KRİTİK**
    -   **Stratejik Önem:** Bu görev, `media-service`'in platformun asenkron dünyasıyla konuşabilmesi için temel altyapıyı oluşturur. Bu olmadan `MEDIA-004` yapılamaz.
    -   **Çözüm Stratejisi:** `main.rs` içinde bir `lapin::Channel` oluşturulmalı ve bu, `AppState` üzerinden tüm `rtp_session_handler`'lara `Arc` ile paylaşılmalıdır.
    -   **Kabul Kriterleri:**
        -   [ ] Servis başladığında, RabbitMQ'ya başarıyla bağlandığına ve `sentiric_events` exchange'ini deklare ettiğine dair bir log görülmelidir.
        -   [ ] `AppState` yapısı, `Arc<lapin::Channel>` içermelidir.
    -   **Tahmini Süre:** ~4-6 Saat

-   **Görev ID: MEDIA-004 - Kayıt Tamamlandığında Olay Yayınlama**
    -   **Durum:** ⬜ **Yapılacak (Bloklandı)**
    -   **Öncelik:** YÜKSEK
    -   **Stratejik Önem:** Çağrı kayıtlarının platform tarafından erişilebilir hale gelmesini sağlar.
    -   **Bağımlılıklar:** `MEDIA-FEAT-03`.
    -   **Kabul Kriterleri:**
        -   [ ] `finalize_and_save_recording` fonksiyonu, S3'e yazma işlemi başarılı olduğunda, RabbitMQ'ya `call.recording.available` tipinde bir olay yayınlamalıdır.
        -   [ ] Yayınlanan olayın payload'u, `call_id` ve doğru S3 `recording_uri`'sini içermelidir.
    -   **Tahmini Süre:** ~2-3 Saat

---

### **FAZ 3: Gelişmiş Medya Yetenekleri (Gelecek Vizyonu)**
-   [ ] **Görev ID: MEDIA-FEAT-02 - İsteğe Bağlı Çağrı Kaydı Dönüştürme ve Sunma**
    -   **Durum:** ⬜ **Planlandı**
    -   **Stratejik Önem:** Yöneticilerin ve kalite ekiplerinin, teknik kayıtları insan kulağına doğal gelen bir formatta dinlemesini sağlar.
    -   **Tahmini Süre:** ~1-2 Gün

-   [ ] **Görev ID: MEDIA-002 - Gelişmiş Codec Desteği (Opus)**
    -   **Durum:** ⬜ **Planlandı**