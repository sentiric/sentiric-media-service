# 🎙️ Sentiric Media Service - Geliştirme Yol Haritası (v6.0 - Stabilite ve Modernizasyon)

Bu belge, media-service'in geliştirme yol haritasını, tamamlanan görevleri ve mevcut öncelikleri tanımlar.

---

### **FAZ 1: Temel Medya Yetenekleri (Tamamlandı)**

-   [x] **Görev ID: MEDIA-CORE-01 - Port Yönetimi**
-   [x] **Görev ID: MEDIA-CORE-02 - Ses Çalma (`PlayAudio`)**
-   [x] **Görev ID: MEDIA-CORE-03 - Canlı Ses Akışı (`RecordAudio`)**
-   [x] **Görev ID: MEDIA-CORE-04 - Kalıcı Kayıt Altyapısı**
-   [x] **Görev ID: MEDIA-FEAT-03 - RabbitMQ Entegrasyonu**
-   [x] **Görev ID: MEDIA-004 - Zenginleştirilmiş Olay Yayınlama**

---

### **FAZ 2: Çift Yönlü Ses Kararlılığı ve Mimari Sağlamlaştırma (Mevcut Odak)**

**Amaç:** Canlı testlerde tespit edilen kritik hataları çözmek, servisi platform standartlarına uygun, dayanıklı ve güvenli bir mimariye kavuşturmak.

-   **Görev ID: MEDIA-BUG-01 - Tek Yönlü Ses ve Bozuk Kayıt Sorununu Giderme (KRİTİK)**
    -   **Durum:** ⬜ **Yapılacak (ÖNCELİK 1)**
    -   **Problem Tanımı:** Son kullanıcıdan gelen sesin STT servisine ve kalıcı kayda bozuk veya hiç ulaşmaması. Bu, diyalog akışını kıran en temel sorundur.
    -   **Çözüm Stratejisi:**
        1.  Gelen RTP paketlerinin çözümlenmesi (decode), yeniden örneklenmesi (resample) ve ses kanallarının birleştirilmesi (mix) adımları, `sentiric-sip-core-service`'teki başarılı codec işleme mantığı referans alınarak ve birim testleri yazılarak dikkatle incelenecektir.
    -   **Kabul Kriterleri:**
        -   [ ] Bir test araması sonrası S3'e kaydedilen `.wav` dosyasında, **hem kullanıcının sesi hem de sistemin sesi** net ve anlaşılır bir şekilde duyulmalıdır.
        -   [ ] `stt-service`, test araması sırasında kullanıcının konuştuğu anlamlı cümleleri doğru bir şekilde metne çevirebilmelidir.

-   **Görev ID: MEDIA-REFACTOR-01 - Dayanıklı Başlatma ve Graceful Shutdown**
    -   **Durum:** ⬜ **Yapılacak (ÖNCELİK 2)**
    -   **Problem Tanımı:** Servis, başlangıçta bağımlılıkları hazır değilse `panic!` ile çökmektedir. Bu, dağıtık ortamlarda kırılgan bir davranıştır.
    -   **Çözüm Stratejisi:** `main.rs` ve `lib.rs` (`run` fonksiyonu), servisin hemen başlayıp arka planda periyodik olarak RabbitMQ ve S3 gibi bağımlılıklara bağlanmayı deneyeceği ve `CTRL+C` ile her an kontrollü bir şekilde kapatılabileceği şekilde yeniden yapılandırılacaktır. `unwrap()` ve `expect()` kullanımları, `Result` döndüren daha güvenli yapılarla değiştirilecektir.

-   **Görev ID: MEDIA-IMPRV-01 - Dockerfile Güvenlik ve Standardizasyonu**
    -   **Durum:** ⬜ **Yapılacak**
    -   **Açıklama:** `Dockerfile`, root kullanıcısıyla çalışmaktadır.
    -   **Kabul Kriterleri:**
        -   [ ] Güvenlik en iyi uygulamalarına uymak için, imaj içinde root olmayan bir `appuser` oluşturulmalı ve uygulama bu kullanıcı ile çalıştırılmalıdır.

---

### **FAZ 3: Gelişmiş Medya Özellikleri (Gelecek Vizyonu)**

-   [ ] **Görev ID: MEDIA-FEAT-01 - Codec Müzakeresi:** Gelen SDP'ye göre G.729 gibi farklı kodekleri destekleme.
-   [ ] **Görev ID: MEDIA-FEAT-02 - Güvenli Medya (SRTP):** Ses akışını uçtan uca şifrelemek için SRTP desteği ekleme.
-   [ ] **Görev ID: MEDIA-FEAT-04 - Anlık Ses Analizi:** Ses akışı üzerinden anlık olarak duygu analizi veya anahtar kelime tespiti yapabilme.