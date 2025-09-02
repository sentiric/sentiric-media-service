# 🎙️ Sentiric Media Service - Geliştirme Yol Haritası (v5.3 - Olay Yayınlama Yeteneği)

Bu belge, `media-service`'in, `sentiric-governance` anayasasında tanımlanan rolünü eksiksiz bir şekilde yerine getirmesi için gereken tüm görevleri listeler.

---

### **FAZ 1: Stabilizasyon ve Temel Yetenekler (Tamamlanmış Görevler)**

Bu faz, platformdaki tüm ses kalitesi sorunlarını çözmüş ve servisi, platformun asenkron dünyasıyla iletişim kurabilen, veri bütünlüğünü sağlayan kararlı bir bileşen haline getirmiştir.

-   [x] **Görev ID: MEDIA-REFACTOR-01 - Merkezi Ses İşleme ve Transcoding Motoru**
    -   **Durum:** ✅ **Tamamlandı**
    -   **Açıklama:** Platformdaki "hızlandırılmış ses" ve cızırtı sorunlarını kökten çözmek için, gelen/giden tüm ses akışlarını standart bir 16kHz LPCM ara formatına dönüştüren bir transcoding katmanı eklendi. Ses kalitesi ve format tutarlılığı garanti altına alındı.

-   [x] **Görev ID: MEDIA-001B - Kalıcı Çağrı Kaydı**
    -   **Durum:** ✅ **Tamamlandı**
    -   **Açıklama:** Çağrıların sesini S3 uyumlu hedeflere (MinIO, R2 vb.) `16kHz WAV` formatında kaydeden `Start/StopRecording` RPC'leri implemente edildi.

-   [x] **Görev ID: DEVOPS-001 - Lokal S3 Simülasyon Ortamı**
    -   **Durum:** ✅ **Tamamlandı**
    -   **Açıklama:** Geliştirme ve test süreçlerini kolaylaştırmak için `docker-compose` dosyalarına MinIO (S3 simülatörü) entegrasyonu yapıldı.

-   [x] **Görev ID: MEDIA-FEAT-03 - RabbitMQ Publisher Entegrasyonu**
    -   **Durum:** ✅ **Tamamlandı**
    -   **Açıklama:** `media-service`'in platformun asenkron olay akışına dahil olabilmesi için RabbitMQ bağlantısı ve `Publisher` yeteneği eklendi. Bu, `MEDIA-004` görevinin ön koşuluydu.

-   [x] **Görev ID: MEDIA-004 - Kayıt Tamamlandığında Olay Yayınlama**
    -   **Durum:** ✅ **Tamamlandı**
    -   **Açıklama:** Bir çağrı kaydı başarıyla S3'e yazıldıktan sonra, `cdr-service`'in bu bilgiyi alıp veritabanına işlemesi için `call.recording.available` olayının RabbitMQ'ya yayınlanması sağlandı.

---

### **FAZ 2: Gelişmiş Medya Yetenekleri (Sıradaki Öncelikler)**

**Amaç:** Platformun çağrı yönetimi ve kullanıcı deneyimi yeteneklerini zenginleştirmek.

-   **Görev ID: MEDIA-FEAT-02 - İsteğe Bağlı Çağrı Kaydı Dönüştürme ve Sunma**
    -   **Durum:** ⬜ **Planlandı**
    -   **Öncelik:** YÜKSEK
    -   **Stratejik Önem:** Yöneticilerin ve kalite ekiplerinin, teknik kayıtları insan kulağına doğal gelen bir formatta dinlemesini sağlar. Bu, `dashboard-ui` gibi arayüzler için kritik bir özelliktir.
    -   **Problem Tanımı:** Kalıcı çağrı kayıtları, teknik doğruluk için 16kHz formatında saklanır ancak bu, insan kulağına "hızlandırılmış" gelebilir.
    -   **Çözüm Stratejisi:** S3'teki ham kaydı anlık olarak işleyip, perdesi düzeltilmiş ve `MP3` gibi yaygın bir formata dönüştürülmüş bir ses akışı sunan yeni bir gRPC endpoint'i (`GetPlayableRecording`) oluşturulacak.
    -   **Tahmini Süre:** ~1-2 Gün

-   **Görev ID: AI-002 - Canlı Ses Akışını Enjekte Etme (`InjectAudio`)**
    -   **Durum:** ⬜ **Planlandı**
    -   **Öncelik:** ORTA
    -   **Açıklama:** Devam eden bir çağrıya harici bir gRPC stream'inden canlı ses enjekte etmek. Bu, "barge-in" (kullanıcı konuşurken AI'ın araya girmesi) gibi gelişmiş diyalog özellikleri için gereklidir.

-   **Görev ID: CONF-001 - Konferans Köprüsü (Conference Bridge)**
    -   **Durum:** ⬜ **Planlandı**
    -   **Öncelik:** ORTA
    -   **Açıklama:** Birden fazla ses akışını tek bir odada birleştirebilen bir konferans köprüsü altyapısı oluşturmak.

---

### **FAZ 3: Protokol ve Güvenlik İyileştirmeleri (Uzun Vade)**

-   [ ] **Görev ID: MEDIA-002 - Gelişmiş Codec Desteği (Opus)**
    -   **Durum:** ⬜ **Planlandı**
    -   **Açıklama:** WebRTC ve yüksek kaliteli ses için kritik olan Opus codec'i için tam transcoding (hem encode hem decode) desteği eklemek.

-   [ ] **Görev ID: SEC-001 - Güvenli Medya Akışı (SRTP Desteği)**
    -   **Durum:** ⬜ **Planlandı**
    -   **Açıklama:** Medya akışını SRTP ile şifreleyerek çağrıların dinlenmesini engellemek.