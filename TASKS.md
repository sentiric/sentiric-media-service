# 🎙️ Sentiric Media Service - Geliştirme Yol Haritası (v5.0 - Merkezi Ses Motoru)

Bu belge, `media-service`'in, `sentiric-governance` anayasasında tanımlanan rolünü eksiksiz bir şekilde yerine getirmesi için gereken tüm görevleri, projenin resmi fazlarına ve aciliyet durumuna göre yeniden düzenlenmiş bir şekilde listeler.

---

### **FAZ 1: Stabilizasyon ve Uçtan Uca Akış Garantisi (KRİTİK GÖREV)**

**Amaç:** Platformdaki tüm ses kalitesi sorunlarını (cızırtı, bozulma, sessiz/yanlış formatta kayıt) kökten çözmek ve `media-service`'i, gelen ve giden tüm ses akışlarının kalitesinden ve formatından sorumlu **tek merkez (Single Source of Truth)** haline getirmek. Bu görev, `agent-service`'in tam diyalog döngüsünü, güvenilir çağrı kaydını ve gelecekteki medya yeteneklerini mümkün kılan temel taştır.

-   [ ] **Görev ID: MEDIA-REFACTOR-01 - Merkezi Ses İşleme ve Transcoding Motoru (KRİTİK & ACİL)**
    -   **Durum:** ⬜ **Yapılacak (TÜM PROJENİN ÖNCELİĞİ)**
    -   **Engelleyici Mi?:** **EVET. TAM DİYALOG AKIŞINI, GÜVENİLİR ÇAĞRI KAYDINI VE ÇOKLU KODEK DESTEĞİNİ TAMAMEN BLOKE EDİYOR.**
    -   **Tahmini Süre:** ~2-3 gün
    -   **Problem Tanımı:** Mevcut durumda, farklı kaynaklardan (PSTN, TTS) gelen sesler, farklı formatlarda (8kHz PCMA/PCMU, 24kHz LPCM) sisteme girmekte ve bu format tutarsızlığı; cızırtılı canlı dinlemeye (STT), bozuk veya tek taraflı çağrı kayıtlarına ve kodek uyumsuzluklarına yol açmaktadır. `media-service`, bu karmaşıklığı yönetmek yerine, bu sorunu diğer servislere yaymaktadır.

    -   **Çözüm Mimarisi: "Ara Format" (Pivot Format) Yaklaşımı**
        `media-service`, bir "ses adaptörü" gibi davranacaktır. Tüm ses işlemleri, yüksek kaliteli bir dahili ara format olan **16kHz, 16-bit, mono LPCM** üzerinden yapılacaktır.
        1.  **Giriş (Decode & Resample):** Gelen tüm ses akışları (örn: 8kHz PCMU), alındığı anda bu 16kHz'lik ara formata dönüştürülecektir.
        2.  **İşleme (İç Akış):** Tüm iç işlemler (canlı akışı STT'ye gönderme, kalıcı kayda ekleme) bu standart ve temiz 16kHz format üzerinden gerçekleştirilecektir.
        3.  **Çıkış (Resample & Encode):** Standart formattaki ses (örn: TTS'ten gelen veya kaydedilmiş anons), hedef sisteme gönderilmeden hemen önce hedefin beklediği formata (örn: telefon için 8kHz PCMA) dönüştürülecektir.

    -   **Uygulama Adımları:**
        -   [ ] **1. `rtp/codecs.rs` Modülü Oluşturma:**
            -   [ ] Tüm G.711 (PCMA/PCMU) ve LPCM dönüşüm mantığı bu merkezi modüle taşınmalıdır.
            -   [ ] `decode_g711_to_lpcm16(payload, codec)`: Gelen 8kHz G.711 verisini 16kHz LPCM'e çeviren bir fonksiyon oluşturulmalıdır.
            -   [ ] `encode_lpcm16_to_g711(samples, codec)`: 16kHz LPCM verisini giden 8kHz G.711'e çeviren bir fonksiyon oluşturulmalıdır.
        -   [ ] **2. `rtp_session_handler`'ı Yeniden Yapılandırma:**
            -   [ ] **Gelen RTP Paketleri:** `socket.recv_from` ile alınan her paket, anında `codecs::decode_g711_to_lpcm16` kullanılarak standart 16kHz LPCM'e dönüştürülmelidir.
            -   [ ] **Canlı Akış (`RecordAudio`):** STT'ye gönderilecek gRPC stream'ine, sadece bu standartlaştırılmış 16kHz LPCM verisi yazılmalıdır.
            -   [ ] **Kalıcı Kayıt (`StartRecording`):** Kayıt havuzuna (`permanent_recording_session.samples`) sadece standart 16kHz LPCM verisi (hem gelen hem giden) eklenmelidir. `WavSpec` her zaman `16000` Hz olarak sabitlenmelidir.
            -   [ ] **Giden RTP Paketleri (`PlayAudio`):**
                -   `send_announcement_from_uri` fonksiyonu, çalınacak sesi (ister WAV, ister Base64) önce standart 16kHz LPCM formatına getirmelidir.
                -   Ardından bu standart veriyi, `codecs::encode_lpcm16_to_g711` kullanarak hedefin beklediği kodeğe çevirip göndermelidir.
                -   **Performans Notu:** `PlayAudio`'nun tetiklediği yoğun RTP gönderme işlemi, ana `tokio` görevlerini bloke etmemelidir. Bu işlem, `tokio::task::spawn_blocking` kullanılarak ayrı bir thread'e taşınmalıdır. Bu, aynı anda gelen RTP paketlerini dinleme ve gRPC stream'ine veri yazma gibi görevlerin kesintiye uğramamasını garanti eder.

    -   **Kabul ve Doğrulama Kriterleri:**
        -   [ ] **Uçtan Uca Test (`end_to_end_call_validator.rs`):** Aşağıdaki senaryoyu eksiksiz ve otomatik olarak doğrulayan bir entegrasyon testi oluşturulmalıdır:
            -   [ ] Test, `PCMA` kodeği ile bir çağrı başlatır ve **16kHz WAV** formatında kalıcı kayıt (`StartRecording`) açar.
            -   [ ] Test, **eş zamanlı olarak** şunları yapar:
                1.  3 saniye boyunca sunucuya **8kHz PCMA** formatında RTP paketleri gönderir (kullanıcıyı simüle eder).
                2.  `RecordAudio` gRPC stream'ini dinler ve gelen ses verisinin **temiz, 16kHz LPCM** formatında olduğunu doğrular.
                3.  Sunucuya, bir anonsu (`welcome.wav`) çalması için `PlayAudio` komutu gönderir.
            -   [ ] Test tamamlandığında, MinIO'dan indirilen kayıt dosyası (`.wav`) programatik olarak analiz edilmeli ve aşağıdaki koşulları sağlamalıdır:
                -   **Format Doğruluğu:** WAV başlığı `16000 Hz`, `16-bit`, `mono` olmalıdır.
                -   **İçerik Bütünlüğü:** Kaydın içinde, hem testin gönderdiği kullanıcı sesinin (PCMA->16k LPCM) hem de sunucunun çaldığı bot anonsunun (WAV->16k LPCM) birleştirilmiş ve temiz bir şekilde bulunduğu kanıtlanmalıdır.
        -   [ ] **Ortam Bağımsızlığı:** Bu testin başarılı olması için gereken tüm ortam yapılandırmaları (örn: `development.env`'deki IP adresleri, Windows Güvenlik Duvarı kuralları) dokümante edilmeli ve testin kendisi bu yapılandırmalara uygun şekilde çalışmalıdır. Test, artık `0 byte` veri almamalı, beklenen miktarda ses verisini başarıyla işlemelidir.

---

### **FAZ 2: Gelişmiş Medya Yetenekleri ve Yönetim**

**Amaç:** Platformun çağrı yönetimi yeteneklerini zenginleştirmek, production ortamına hazırlamak ve daha güvenli hale getirmek.

-   [x] **Görev ID: MEDIA-001B - Kalıcı Çağrı Kaydı**
    -   **Açıklama:** Çağrı sesini bir dosyaya kaydetme özelliği.
    -   **Durum:** ✅ **Tamamlandı**
    -   **Güncelleme Notu (29.08.2025):** Bu özellik, S3-uyumlu nesne depolama hedeflerini (AWS S3, Cloudflare R2, MinIO vb.) destekleyecek şekilde `force_path_style` düzeltmesi ile tam fonksiyonel hale getirildi.

-   [x] **Görev ID: DEVOPS-001 - Lokal S3 Simülasyon Ortamı (YENİ GÖREV)**
    -   **Açıklama:** Geliştirme ve test süreçlerini hızlandırmak için `docker-compose`'a MinIO (S3 simülatörü) entegrasyonu yapmak.
    -   **Durum:** ✅ **Tamamlandı**
    -   **Kabul Kriterleri:**
        -   [x] `docker-compose` içinde `minio` servisi tanımlandı.
        -   [x] `media-service`, ortam değişkenleri aracılığıyla yerel MinIO hedefine kayıt yapabiliyor.
        -   [x] Altyapı, farklı profillerde (lokal vs cloud) farklı S3 hedeflerini destekleyecek şekilde esnek yapılandırıldı.


-   [ ] **Görev ID: MEDIA-004 - Kayıt Tamamlandığında Olay Yayınlama (YÜKSEK ÖNCELİK)**
    -   **Durum:** ⬜ Planlandı
    -   **Bağımlılık:** `AGENT-DIAG-01`'in tamamlanmasına bağlı.
    -   **Tahmini Süre:** ~2-3 saat
    -   **Açıklama:** Bir çağrı kaydı başarıyla S3/MinIO'ya yazıldıktan sonra, bu kaydın URL'ini içeren bir `call.recording.available` olayını RabbitMQ'ya yayınlamak. Bu, `cdr-service`'in kaydı ilgili çağrıyla ilişkilendirmesi için kritiktir.
    -   **Kabul Kriterleri:**
        -   [ ] `src/rtp/session.rs` içindeki `finalize_and_save_recording` fonksiyonu, S3'e yazma işlemi başarılı olduğunda `RabbitMQ`'ya `sentiric-contracts`'te tanımlı `CallRecordingAvailableEvent` formatında bir olay yayınlamalıdır.
        -   [ ] Olayın `recording_uri` alanı, MinIO'daki dosyanın tam S3 URI'sini içermelidir.
        -   [ ] RabbitMQ yönetim arayüzünden bu olayın doğru bir şekilde yayınlandığı gözlemlenmelidir.

-   [ ] **Görev ID: MEDIA-BUG-01 - Boş Ses Kaydı Kök Neden Analizi (DOĞRULAMA GÖREVİ)**
    -   **Durum:** ⬜ Planlandı
    -   **Bağımlılık:** `AGENT-BUG-02`'nin çözülmesine bağlı.
    -   **Açıklama:** `agent-service`'teki hata düzeltildikten sonra, çağrı kaydının artık boş olmadığını, gerçek ses verisi içerdiğini doğrulamak. Eğer sorun devam ederse, `rtp_session_handler` içindeki kayıt mantığını derinlemesine incelemek.
    -   **Kabul Kriterleri:**
        -   [ ] Başarılı bir test çağrısından sonra MinIO'dan indirilen `.wav` dosyası, ses içermelidir.
        -   [ ] Eğer hala boşsa, `rtp_session_handler`'ın RTP paketlerini doğru bir şekilde `permanent_recording_session.samples` vektörüne ekleyip eklemediği loglarla ve debug ile kontrol edilmelidir.

-   [ ] **Görev ID: SEC-001 - Güvenli Medya Akışı (SRTP Desteği)**
    -   **Açıklama:** Medya akışını SRTP ile şifreleyerek çağrıların dinlenmesini engellemek.
    -   **Kabul Kriterleri:**
        -   [ ] `AllocatePort` RPC'si veya yeni bir `AllocateSecurePort` RPC'si, SRTP için gerekli şifreleme anahtarlarını (`master key` ve `salt`) alabilmelidir.
        -   [ ] `rtp_session_handler`, `webrtc-rs/srtp` gibi bir kütüphane kullanarak RTP paketlerini şifrelemeli/deşifre etmelidir.
        -   [ ] **Test:** Bir test çağrısı sırasında Wireshark ile ağ trafiği dinlendiğinde, RTP paketlerinin payload'ının **okunamaz (şifreli)** olduğu kanıtlanmalıdır.

-   [ ] **Görev ID: OBS-001 - Metriklerin Detaylandırılması (YENİ GÖREV)**
    -   **Açıklama:** Servisin anlık durumu ve performansı hakkında daha fazla bilgi edinmek için Prometheus metriklerini zenginleştirmek.
    -   **Durum:** ⬜ Planlandı.
    -   **Kabul Kriterleri:**
        -   [ ] `sentiric_media_port_pool_available_count` (kullanılabilir port sayısı) anlık olarak raporlanmalı.
        -   [ ] `sentiric_media_port_pool_quarantined_count` (karantinadaki port sayısı) anlık olarak raporlanmalı.
        -   [ ] `sentiric_media_recording_saved_total` (başarıyla kaydedilen toplam çağrı sayısı) sayacı eklenmeli. Bu sayaç, `storage_type` (file, s3) etiketiyle ayrıştırılabilmeli.
        -   [ ] `sentiric_media_recording_failed_total` (kaydedilemeyen çağrı sayısı) sayacı eklenmeli.


-   [ ] **Görev ID: MEDIA-FEAT-02 - İsteğe Bağlı Çağrı Kaydı Dönüştürme ve Sunma (YÜKSEK ÖNCELİK)**
    -   **Durum:** ⬜ Planlandı
    -   **Engelleyici Mi?:** HAYIR, ama `cdr-service` gibi arayüz servislerinin kullanıcıya doğal sesli kayıt dinletme özelliğini doğrudan etkiler.
    -   **Tahmini Süre:** ~1-2 gün
    -   **Problem Tanımı:** Kalıcı çağrı kayıtları (`StartRecording`), teknik doğruluk ve STT uyumluluğu için "Ara Format" (`16kHz LPCM`) ile saklanmaktadır. Bu format, telefon hattı fiziğini (8kHz -> 16kHz dönüşümü) simüle ettiği için insan kulağına doğal gelmeyen, perdesi yüksek ("hızlı") bir sese sahiptir. Bu kayıtların doğrudan bir kullanıcıya (örn: bir yönetici) dinletilmesi, kötü bir kullanıcı deneyimi yaratır.
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
            -   [ ] **Perde Düzeltme (Pitch Correction):** Bu `16kHz` LPCM verisi, `rubato` kütüphanesi kullanılarak orijinal perdesine geri getirilmelidir. Bu, tipik olarak 16kHz -> 8kHz (orijinal frekans bandına dönüş) ve ardından tekrar 8kHz -> 16kHz (kaliteyi koruyarak) gibi bir yeniden örnekleme zinciriyle veya daha uygun bir pitch-shifting tekniğiyle gerçekleştirilebilir. Amaç, "Chipmunk etkisini" ortadan kaldırmaktır.
            -   [ ] **Format Dönüşümü:** Perdesi düzeltilmiş `LPCM` verisi, istekte belirtilen `target_format`'a (örn: `audio/mpeg` için MP3) anlık olarak encode edilmelidir. (Başlangıç için sadece `audio/wav` desteklenebilir).
            -   [ ] Encode edilen ses verisi, uygun boyutlarda parçalara (chunks) ayrılarak gRPC stream'i üzerinden istemciye gönderilmelidir.
    -   **Kabul ve Doğrulama Kriterleri:**
        -   [ ] Yeni bir test istemcisi (`playable_recording_client.rs`) oluşturulmalıdır.
        -   [ ] Bu istemci, `end_to_end_call_validator` tarafından oluşturulmuş perdesi yüksek bir kaydın URI'sini `GetPlayableRecording` RPC'sine göndermelidir.
        -   [ ] Gelen ses akışı bir dosyaya yazılmalı ve bu dosya dinlendiğinde, sesin perdesinin doğal ve anlaşılır olduğu, hızlanma etkisinin ortadan kalktığı doğrulanmalıdır.
        -   [ ] Stream'in başarılı bir şekilde tamamlandığı ve istemcinin tüm ses verisini aldığı teyit edilmelidir.

---

### **FAZ 3: Gelecek Vizyonu ve Genişletilebilirlik**

**Amaç:** Platformu WebRTC gibi modern teknolojilere ve konferans gibi karmaşık senaryolara hazırlamak.

-   [ ] **Görev ID: MEDIA-002 - Gelişmiş Codec Desteği (Opus)**
    -   **Açıklama:** WebRTC ve yüksek kaliteli ses için kritik olan Opus codec'i için tam transcoding (hem encode hem decode) desteği eklemek.
    -   **Kabul Kriterleri:**
        -   [ ] Servis, G.711 (PCMU) formatında gelen bir RTP akışını Opus formatına çevirip gönderebilmelidir.
        -   [ ] Servis, Opus formatında gelen bir RTP akışını G.711 (PCMU) formatına çevirip gönderebilmelidir.
        -   [ ] Transcoding işlemi, ses kalitesinde minimum kayıpla ve kabul edilebilir bir gecikme (latency) ile gerçekleşmelidir.

-   [ ] **Görev ID: AI-002 - Canlı Ses Akışını Enjekte Etme (`InjectAudio`)**
    -   **Açıklama:** Devam eden bir çağrıya harici bir gRPC stream'inden canlı ses enjekte etmek. Bu, "barge-in" (kullanıcı konuşurken AI'ın araya girmesi) gibi gelişmiş diyalog özellikleri için gereklidir.
    -   **Durum:** ⬜ Planlandı.

-   [ ] **Görev ID: CONF-001 - Konferans Köprüsü (Conference Bridge)**
    -   **Açıklama:** Birden fazla ses akışını tek bir odada birleştirebilen bir konferans köprüsü altyapısı oluşturmak.
    -   **Durum:** ⬜ Planlandı.