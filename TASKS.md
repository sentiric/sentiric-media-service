# 🎙️ Sentiric Media Service - Geliştirme Yol Haritası (v3.2 - Tamamlanmış Sürüm)

Bu belge, `sentiric-media-service`'in, `sentiric-governance` anayasasında tanımlanan rolünü eksiksiz bir şekilde yerine getirmesi için gereken tüm görevleri, projenin resmi fazlarına ve ölçülebilir kabul kriterlerine göre düzenlenmiş bir şekilde listeler.

---

### **Faz 0: Temel Sağlamlaştırma (Foundation Refactoring)**

**Amaç:** Mevcut kod tabanını, anayasanın gerektirdiği esnek ve yönetilebilir yapıya kavuşturmak.

-   [ ] **Görev ID: CORE-001 - Merkezi Durum Yönetimi (`AppState`)**
    -   **Açıklama:** Tüm paylaşılan durumları tek bir `Arc<AppState>` yapısı altında birleştirmek.
    -   **Kabul Kriterleri:**
        -   [ ] `AppState` struct'ı oluşturuldu ve `PortManager` ile `AudioCache`'i içeriyor.
        -   [ ] `MyMediaService`, `Arc<AppState>`'i tek bir parametre olarak alıyor.
        -   [ ] `allocate_port` gibi metodlar, `self.app_state.port_manager` üzerinden duruma erişiyor.

-   [ ] **Görev ID: CORE-002 - Katmanlı Konfigürasyon**
    -   **Açıklama:** `figment` kütüphanesi ile `config.toml` ve `.env` dosyalarını birleştiren bir yapıya geçmek.
    -   **Kabul Kriterleri:**
        -   [ ] Projede varsayılan ayarları içeren bir `config.default.toml` dosyası var.
        -   [ ] `.env` dosyasındaki bir değişken, `toml` dosyasındaki aynı değişkeni ezebiliyor (override).
        -   [ ] Servis, sadece `toml` veya sadece `.env` ile de çalışabiliyor.

---

### **Faz 1 & 1.5: Güvenli ve Gözlemlenebilir Omurga**

**Amaç:** Servisin, platformun temel güvenlik ve izlenebilirlik standartlarını karşılamasını sağlamak.

-   [x] ~~**Görev ID: SEC-002 - Servisler Arası Güvenlik (mTLS)**~~ (✅ Tamamlandı)

-   [x] **Görev ID: OBS-001 - Standart ve Ortama Duyarlı Loglama**
    -   **Açıklama:** Loglamayı, `OBSERVABILITY_STANDARD.md`'ye tam uyumlu hale getirmek.
    -   **Kabul Kriterleri:**
        -   [ ] `ENV=development` olarak çalıştırıldığında, loglar konsolda renkli ve insan dostu formatta basılıyor.
        -   [ ] `ENV=production` olarak çalıştırıldığında, loglar **JSON formatında** basılıyor.
        -   [ ] JSON logları, `service`, `level`, `timestamp`, `message`, `trace_id`, `call_id` alanlarını zorunlu olarak içeriyor.

-   [x] **Görev ID: OBS-002 - Prometheus Metrikleri Endpoint'i**
    -   **Açıklama:** `/metrics` endpoint'i üzerinden Prometheus formatında metrikler sunmak.
    -   **Kabul Kriterleri:**
        -   [ ] Servise `curl localhost:<port>/metrics` isteği atıldığında geçerli Prometheus metrikleri dönüyor.
        -   [ ] Sunulan metrikler arasında en az `sentiric_media_active_sessions` ve `sentiric_media_grpc_requests_total` bulunuyor.

-   [x] **Görev ID: OBS-003 - Dağıtık İzleme Entegrasyonu**
    -   **Açıklama:** Gelen gRPC isteklerindeki `trace_id`'yi yakalayıp loglara yaymak.
    -   **Kabul Kriterleri:**
        -   [ ] gRPC isteği `trace_id` metadata'sı ile geldiğinde, bu ID'nin tüm log satırlarında göründüğü doğrulanmalı.
        -   [ ] `trace_id` gelmediğinde, sistemin yeni bir tane oluşturduğu veya alanı boş bıraktığı doğrulanmalı.

---

### **Faz 2: Fonksiyonel İskelet**

**Amaç:** Platformun temel çağrı akışını tamamlayacak çekirdek medya yeteneklerini hayata geçirmek.

-   [x] ~~**Görev ID: MEDIA-000 - Temel Port ve Session Yönetimi**~~ (✅ Tamamlandı)
-   [x] ~~**Görev ID: MEDIA-001A - Medya Oynatma (`PlayAudio`)**~~ (✅ Tamamlandı)

-   [ ] **Görev ID: SEC-001 - Güvenli Medya Akışı (SRTP Desteği)**
    -   **Açıklama:** Medya akışını SRTP ile şifrelemek.
    -   **Kabul Kriterleri:**
        -   [ ] `AllocatePort` veya yeni bir RPC, şifreleme anahtarlarını alabiliyor.
        -   [ ] `rtp_session_handler`, `webrtc-rs/srtp` gibi bir kütüphane kullanarak RTP paketlerini şifreliyor/deşifre ediyor.
        -   [ ] **Test:** Bir test çağrısı sırasında Wireshark ile ağ trafiği dinlendiğinde, RTP paketlerinin payload'ının **okunamaz (şifreli)** olduğu kanıtlanmalı.
        
-   [x] **Görev ID: MEDIA-001B - Kalıcı Çağrı Kaydı**
    -   **Açıklama:** Çağrı sesini bir dosyaya kaydedip S3 gibi harici depolama sistemlerine yükleme özelliği.
    -   **Kabul Kriterleri:**
        -   [ ] Yeni bir `StartRecording` RPC'si, `output_uri` (`s3://...` veya `file:///...`) ve `format` (`wav`, `mp3`) gibi parametreler alıyor.
        -   [ ] Servis, gelen RTP akışını belirtilen formatta bir dosyaya yazıyor.
        -   [ ] Kayıt tamamlandığında, dosya belirtilen `output_uri`'ye yükleniyor.
        -   [ ] Yükleme işlemi, ana medya işleme thread'ini bloke etmeyecek şekilde asenkron olarak yapılıyor.

---

### **Faz 3: Canlanan Platform (AI Yetenekleri)**

**Amaç:** `media-service`'i, platformun akıllı diyalog kurmasını sağlayacak AI entegrasyon yetenekleriyle donatmak.

-   [ ] **Görev ID: AI-001 - Canlı Ses Akışını Çoğaltma (`StreamAudio`)**
    -   **Açıklama:** Gelen RTP akışını anlık olarak bir gRPC stream'i olarak dışarıya aktarmak.
    -   **Kabul Kriterleri:**
        -   [ ] Bir test istemcisi `StreamAudio` RPC'sini çağırdığında, geçerli bir gRPC stream'i alıyor.
        -   [ ] Simüle edilen bir RTP kaynağından gelen ses paketleri, milisaniyeler içinde bu gRPC stream'inden okunabiliyor.
        -   [ ] Bu işlem sırasında, orijinal RTP akışının karşı tarafa iletiminde **kesinti olmadığı** doğrulanmalı.

-   [ ] **Görev ID: AI-002 - Canlı Ses Akışını Enjekte Etme (`InjectAudio`)**
    -   **Açıklama:** Devam eden bir çağrıya harici bir gRPC stream'inden canlı ses enjekte etmek.
    -   **Kabul Kriterleri:**
        -   [ ] Bir test istemcisi, `InjectAudio` RPC'sine PCM formatında ses verisi gönderdiğinde, çağrının diğer ucundaki dinleyici bu sesi RTP akışı olarak duyabiliyor.
        -   [ ] Enjeksiyon sırasında, karşı taraftan gelen sesin akışı etkilenmiyor (sesler miksleniyor veya sırayla çalınıyor).

---

### **Faz 4 ve Ötesi: Akıllı ve Genişletilebilir Platform**
*(Bu fazın görevleri, önceki fazlar tamamlandıktan sonra detaylandırılacaktır)*

-   [ ] **Görev ID: CONF-001 - Konferans Köprüsü (Conference Bridge)**
-   [ ] **Görev ID: RTC-001 - WebRTC Desteği**
-   [ ] **Görev ID: SCALE-001 - Harici Durum Yönetimi (Stateless Architecture)**
-   [ ] **Görev ID: MEDIA-002 - Gelişmiş Codec Desteği (Opus)**
    -   **Açıklama:** WebRTC ve yüksek kaliteli ses için kritik olan Opus codec'i için tam transcoding (hem encode hem decode) desteği eklemek.
    -   **Kabul Kriterleri:**
        -   [ ] Servis, G.711 formatında gelen bir RTP akışını Opus formatına çevirip gönderebiliyor.
        -   [ ] Servis, Opus formatında gelen bir RTP akışını G.711 formatına çevirip gönderebiliyor.
        -   [ ] Transcoding işlemi, ses kalitesinde minimum kayıpla ve kabul edilebilir bir gecikme (latency) ile gerçekleşiyor.