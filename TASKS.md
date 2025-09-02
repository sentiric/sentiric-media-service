# 🎙️ Sentiric Media Service - Geliştirme Yol Haritası (v5.1 - Stabilize Edilmiş Ses Motoru)

Bu belge, `media-service`'in, `sentiric-governance` anayasasında tanımlanan rolünü eksiksiz bir şekilde yerine getirmesi için gereken tüm görevleri, projenin resmi fazlarına ve aciliyet durumuna göre yeniden düzenlenmiş bir şekilde listeler.

---

### **FAZ 1: Stabilizasyon ve Uçtan Uca Akış Garantisi (KRİTİK GÖREV)**

**Amaç:** Platformdaki tüm ses kalitesi sorunlarını (cızırtı, bozulma, sessiz/yanlış formatta kayıt) kökten çözmek ve `media-service`'i, gelen ve giden tüm ses akışlarının kalitesinden ve formatından sorumlu **tek merkez (Single Source of Truth)** haline getirmek. Bu görev, `agent-service`'in tam diyalog döngüsünü, güvenilir çağrı kaydını ve gelecekteki medya yeteneklerini mümkün kılan temel taştır.

-   [x] **Görev ID: MEDIA-REFACTOR-01 - Merkezi Ses İşleme ve Transcoding Motoru (KRİTİK & ACİL)**
    -   **Durum:** ✅ **Tamamlandı ve Doğrulandı**
    -   **Engelleyici Mi?:** **EVET. TAM DİYALOG AKIŞINI, GÜVENİLİR ÇAĞRI KAYDINI VE ÇOKLU KODEK DESTEĞİNİ TAMAMEN BLOKE EDİYOR.**
    -   **Problem Tanımı:** Mevcut durumda, farklı kaynaklardan (PSTN, TTS) gelen sesler, farklı formatlarda (8kHz PCMA/PCMU, 24kHz LPCM) sisteme girmekte ve bu format tutarsızlığı; cızırtılı canlı dinlemeye (STT), bozuk veya tek taraflı çağrı kayıtlarına ve kodek uyumsuzluklarına yol açmaktadır. `media-service`, bu karmaşıklığı yönetmek yerine, bu sorunu diğer servislere yaymaktadır.

    -   **Çözüm Mimarisi: "Ara Format" (Pivot Format) Yaklaşımı**
        `media-service`, bir "ses adaptörü" gibi davranacaktır. Tüm ses işlemleri, yüksek kaliteli bir dahili ara format olan **16kHz, 16-bit, mono LPCM** üzerinden yapılacaktır.
        1.  **Giriş (Decode & Resample):** Gelen tüm ses akışları (örn: 8kHz PCMU), alındığı anda bu 16kHz'lik ara formata dönüştürülecektir.
        2.  **İşleme (İç Akış):** Tüm iç işlemler (canlı akışı STT'ye gönderme, kalıcı kayda ekleme) bu standart ve temiz 16kHz format üzerinden gerçekleştirilecektir.
        3.  **Çıkış (Resample & Encode):** Standart formattaki ses (örn: TTS'ten gelen veya kaydedilmiş anons), hedef sisteme gönderilmeden hemen önce hedefin beklediği formata (örn: telefon için 8kHz PCMA) dönüştürülecektir.

    -   **Uygulama Adımları:**
        -   [x] **1. `rtp/codecs.rs` Modülü Oluşturma:**
            -   [x] Tüm G.711 (PCMA/PCMU) ve LPCM dönüşüm mantığı bu merkezi modüle taşınmalıdır.
            -   [x] `decode_g711_to_lpcm16(payload, codec)`: Gelen 8kHz G.711 verisini 16kHz LPCM'e çeviren bir fonksiyon oluşturulmalıdır.
            -   [x] `encode_lpcm16_to_g711(samples, codec)`: 16kHz LPCM verisini giden 8kHz G.711'e çeviren bir fonksiyon oluşturulmalıdır.
        -   [x] **2. `rtp_session_handler`'ı Yeniden Yapılandırma:**
            -   [x] **Gelen RTP Paketleri:** `socket.recv_from` ile alınan her paket, anında `codecs::decode_g711_to_lpcm16` kullanılarak standart 16kHz LPCM'e dönüştürülmelidir.
            -   [x] **Canlı Akış (`RecordAudio`):** STT'ye gönderilecek gRPC stream'ine, sadece bu standartlaştırılmış 16kHz LPCM verisi yazılmalıdır.
            -   [x] **Kalıcı Kayıt (`StartRecording`):** Kayıt havuzuna (`permanent_recording_session.samples`) sadece standart 16kHz LPCM verisi (hem gelen hem giden) eklenmelidir. `WavSpec` her zaman `16000` Hz olarak sabitlenmelidir.
            -   [x] **Giden RTP Paketleri (`PlayAudio`):**
                -   [x] `send_announcement_from_uri` fonksiyonu, çalınacak sesi (ister WAV, ister Base64) önce standart 16kHz LPCM formatına getirmelidir.
                -   [x] Ardından bu standart veriyi, `codecs::encode_lpcm16_to_g711` kullanarak hedefin beklediği kodeğe çevirip göndermelidir.
                -   [x] **Performans Notu:** `PlayAudio`'nun tetiklediği yoğun RTP gönderme işlemi, ana `tokio` görevlerini bloke etmemelidir. Bu işlem, `tokio::task::spawn_blocking` kullanılarak ayrı bir thread'e taşınmalıdır. Bu, aynı anda gelen RTP paketlerini dinleme ve gRPC stream'ine veri yazma gibi görevlerin kesintiye uğramamasını garanti eder.

    -   **Kabul ve Doğrulama Kriterleri:**
        -   [x] **Uçtan Uca Test (`end_to_end_call_validator.rs`):** Aşağıdaki senaryoyu eksiksiz ve otomatik olarak doğrulayan bir entegrasyon testi oluşturulmalıdır:
            -   [x] Test, `PCMU` kodeği ile bir çağrı başlatır ve **16kHz WAV** formatında kalıcı kayıt (`StartRecording`) açar.
            -   [x] Test, **eş zamanlı olarak** şunları yapar:
                1.  [x] Belirtilen süre boyunca sunucuya **8kHz PCMU** formatında RTP paketleri gönderir (kullanıcıyı simüle eder).
                2.  [x] `RecordAudio` gRPC stream'ini dinler ve gelen ses verisinin **temiz, 16kHz LPCM** formatında olduğunu doğrular.
                3.  [x] Sunucuya, bir anonsu (`welcome.wav`) çalması için `PlayAudio` komutu gönderir.
            -   [x] Test tamamlandığında, MinIO'dan indirilen kayıt dosyası (`.wav`) programatik olarak analiz edilmeli ve aşağıdaki koşulları sağlamalıdır:
                -   [x] **Format Doğruluğu:** WAV başlığı `16000 Hz`, `16-bit`, `mono` olmalıdır.
                -   [x] **İçerik Bütünlüğü:** Kaydın içinde, hem testin gönderdiği kullanıcı sesinin (PCMU->16k LPCM) hem de sunucunun çaldığı bot anonsunun (WAV->16k LPCM) birleştirilmiş ve temiz bir şekilde bulunduğu kanıtlanmalıdır.
        -   [x] **Ortam Bağımsızlığı:** Bu testin başarılı olması için gereken tüm ortam yapılandırmaları `docker-compose.test.yml` ve `.env.test` dosyaları ile sağlanmıştır. Test, beklenen miktarda ses verisini başarıyla işlemektedir.

---
### **FAZ 2: Platform Entegrasyonu ve Veri Akışı (Mevcut Odak)**

-   [x] **Görev ID: MEDIA-001B - Kalıcı Çağrı Kaydı**
    -   **Açıklama:** Çağrı sesini bir dosyaya kaydetme özelliği.
    -   **Durum:** ✅ **Tamamlandı**
    -   **Güncelleme Notu (01.09.2025):** Bu özellik, S3-uyumlu nesne depolama hedeflerini (AWS S3, Cloudflare R2, MinIO vb.) destekleyecek şekilde `force_path_style` düzeltmesi ile tam fonksiyonel hale getirildi.

-   [x] **Görev ID: DEVOPS-001 - Lokal S3 Simülasyon Ortamı**
    -   **Açıklama:** Geliştirme ve test süreçlerini hızlandırmak için `docker-compose`'a MinIO (S3 simülatörü) entegrasyonu yapmak.
    -   **Durum:** ✅ **Tamamlandı**
    -   **Kabul Kriterleri:**
        -   [x] `docker-compose` içinde `minio` servisi tanımlandı.
        -   [x] `media-service`, ortam değişkenleri aracılığıyla yerel MinIO hedefine kayıt yapabiliyor.
        -   [x] Altyapı, farklı profillerde (lokal vs cloud) farklı S3 hedeflerini destekleyecek şekilde esnek yapılandırıldı.


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

-   [ ] **Görev ID: MEDIA-FEAT-02 - İsteğe Bağlı Çağrı Kaydı Dönüştürme ve Sunma (YENİ GÖREV - YÜKSEK ÖNCELİK)**
    -   **Durum:** ⬜ **Planlandı**
    -   **Engelleyici Mi?:** HAYIR, ama `cdr-service` gibi arayüz servislerinin kullanıcıya doğal sesli kayıt dinletme özelliğini doğrudan etkiler.
    -   **Tahmini Süre:** ~1-2 gün
    -   **Problem Tanımı:** Kalıcı çağrı kayıtları, teknik doğruluk ve STT uyumluluğu için "Ara Format" (`16kHz LPCM`) ile saklanmaktadır. Bu format, telefon hattı fiziğini (8kHz -> 16kHz dönüşümü) simüle ettiği için insan kulağına doğal gelmeyen, perdesi yüksek ("hızlı") bir sese sahiptir. Bu kayıtların doğrudan bir kullanıcıya (örn: bir yönetici) dinletilmesi, kötü bir kullanıcı deneyimi yaratır.
    -   **Çözüm Mimarisi: "Sunum Katmanı Dönüşümü"**
        `media-service`, S3'te depolanan ham ve teknik kaydı değiştirmeden, istendiği anda "dinlenebilir" bir formata dönüştüren yeni bir gRPC endpoint'i sunacaktır. Bu, platformdaki tüm ses işleme mantığını tek bir merkezde toplar.
    -   **Uygulama Adımları:**
        -   [ ] **1. `sentiric-contracts` Güncellemesi:**
            -   [ ] `media_service.proto` içine yeni bir `GetPlayableRecording` RPC'si eklenmelidir.
            -   [ ] `GetPlayableRecordingRequest` (içinde `string recording_uri`, `string target_format`) ve `GetPlayableRecordingResponse` (içinde `bytes audio_chunk`) mesajları tanımlanmalıdır.
            -   [ ] Bu RPC, `stream` olarak `GetPlayableRecordingResponse` döndürmelidir.
        -   [ ] **2. `media-service` Implementasyonu (`grpc/service.rs`):**
            -   [ ] `GetPlayableRecording` fonksiyonu implemente edilmelidir.
            -   [ ] Fonksiyon, `recording_uri`'yi kullanarak S3'ten ilgili ham WAV dosyasını indirmelidir.
            -   [ ] İndirilen WAV verisi `16kHz LPCM` sample'larına ayrıştırılmalıdır.
            -   [ ] **Perde Düzeltme (Pitch Correction):** Bu `16kHz` LPCM verisi, `rubato` kütüphanesi kullanılarak orijinal perdesine geri getirilmelidir. Amaç, "Chipmunk etkisini" ortadan kaldırmaktır.
            -   [ ] **Format Dönüşümü:** Perdesi düzeltilmiş `LPCM` verisi, istekte belirtilen `target_format`'a (örn: `audio/mpeg` için MP3) anlık olarak encode edilmelidir.
            -   [ ] Encode edilen ses verisi, uygun boyutlarda parçalara (chunks) ayrılarak gRPC stream'i üzerinden istemciye gönderilmelidir.
    -   **Kabul ve Doğrulama Kriterleri:**
        -   [ ] Yeni bir test istemcisi (`playable_recording_client.rs`) oluşturulmalıdır.
        -   [ ] Bu istemci, `end_to_end_call_validator` tarafından oluşturulmuş perdesi yüksek bir kaydın URI'sini `GetPlayableRecording` RPC'sine göndermelidir.
        -   [ ] Gelen ses akışı bir dosyaya yazılmalı ve bu dosya dinlendiğinde, sesin perdesinin doğal ve anlaşılır olduğu, hızlanma etkisinin ortadan kalktığı doğrulanmalıdır.

-   [ ] **Görev ID: SEC-001 - Güvenli Medya Akışı (SRTP Desteği)**
    -   **Açıklama:** Medya akışını SRTP ile şifreleyerek çağrıların dinlenmesini engellemek.

-   [ ] **Görev ID: OBS-001 - Metriklerin Detaylandırılması**
    -   **Açıklama:** Servisin anlık durumu ve performansı hakkında daha fazla bilgi edinmek için Prometheus metriklerini zenginleştirmek.

---

### **FAZ 3: Gelecek Vizyonu ve Genişletilebilirlik**

**Amaç:** Platformu WebRTC gibi modern teknolojilere ve konferans gibi karmaşık senaryolara hazırlamak.

-   [ ] **Görev ID: MEDIA-002 - Gelişmiş Codec Desteği (Opus)**
    -   **Açıklama:** WebRTC ve yüksek kaliteli ses için kritik olan Opus codec'i için tam transcoding (hem encode hem decode) desteği eklemek.

-   [ ] **Görev ID: AI-002 - Canlı Ses Akışını Enjekte Etme (`InjectAudio`)**
    -   **Açıklama:** Devam eden bir çağrıya harici bir gRPC stream'inden canlı ses enjekte etmek. Bu, "barge-in" (kullanıcı konuşurken AI'ın araya girmesi) gibi gelişmiş diyalog özellikleri için gereklidir.

-   [ ] **Görev ID: CONF-001 - Konferans Köprüsü (Conference Bridge)**
    -   **Açıklama:** Birden fazla ses akışını tek bir odada birleştirebilen bir konferans köprüsü altyapısı oluşturmak.

---
*Not: `MEDIA-BUG-01` görevi, `MEDIA-REFACTOR-01`'in tamamlanmasıyla birlikte kök nedenin çözüldüğü ve artık geçerli olmadığı anlaşıldığı için listeden kaldırılmıştır.*