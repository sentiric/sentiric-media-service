# 🎙️ Sentiric Media Service - Geliştirme Yol Haritası (v4.2)

Bu belge, `media-service`'in, `sentiric-governance` anayasasında tanımlanan rolünü eksiksiz bir şekilde yerine getirmesi için gereken tüm görevleri, projenin resmi fazlarına ve aciliyet durumuna göre yeniden düzenlenmiş bir şekilde listeler.

---

### **FAZ 1: Stabilizasyon ve Uçtan Uca Akış Desteği (ACİL ÖNCELİK)**

**Amaç:** Canlı çağrı akışının çalışmasını engelleyen veya zorlaştıran temel sorunları gidermek ve `agent-service`'in tam diyalog döngüsünü tamamlaması için gereken kritik yetenekleri sağlamak.

-   [x] **Görev ID: MEDIA-003 - Fazla Konuşkan Loglamayı Düzeltme (KRİTİK & ACİL)**
    -   **Açıklama:** `src/lib.rs` dosyasındaki `tracing` yapılandırmasını, `OBSERVABILITY_STANDARD.md`'ye uygun hale getirerek `INFO` seviyesindeki gereksiz `enter/exit` loglarını kaldır.
    -   **Durum:** ✅ **Tamamlandı** (Mevcut kodda doğrulandı).

-   [x] **Görev ID: AI-001 - Canlı Ses Akışını Çoğaltma (`RecordAudio`)**
    -   **Açıklama:** Gelen RTP akışını anlık olarak bir gRPC stream'i olarak `agent-service`'e aktarmak. Bu, canlı STT entegrasyonu için **temel gereksinimdir**.
    -   **Durum:** ✅ **Tamamlandı**
    -   **Kabul Kriterleri:**
        -   [x] `RecordAudio` RPC'si, gelen RTP (PCMU) paketlerini çözmeli ve içindeki ham ses verisini, `sentiric-contracts`'te tanımlanan **standart bir formatta (örn: 16kHz, 16-bit mono PCM)** `AudioFrame` mesajları olarak gRPC stream'ine yazmalıdır.
        -   [x] `examples/live_audio_client.rs` test istemcisi, bu stream'i tüketerek anlık ses verisini alabildiğini kanıtlamıştır.
        -   [ ] Bu işlem sırasında, orijinal RTP akışının karşı tarafa iletiminde **kesinti olmadığı** doğrulanmalıdır. *(Not: Mevcut yapıda ses akışını "çoğaltmıyoruz", sadece dinliyoruz. Gerçek bir çağrıda sesi hem STT'ye hem de karşı tarafa göndermek için mimariyi ileride geliştirmemiz gerekebilir. Şimdilik bu kabul kriteri geçerli değil.)*
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