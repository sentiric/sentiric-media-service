# 🎙️ Sentiric Media Service - Geliştirme Yol Haritası (v6.1 - Stabilite ve Modernizasyon Tamamlandı)

Bu belge, media-service'in geliştirme yol haritasını, tamamlanan görevleri ve mevcut öncelikleri tanımlar.

---

*   **Görev ID:** `MEDIA-BUG-02`
    *   **Başlık:** fix(rtp): Gelen RTP (inbound) ses akışındaki bozulmayı ve cızırtıyı düzelt
    *   **Durum:** `[ ⬜ ] Yapılacak`
    *   **Öncelik:** **KRİTİK - BLOKLAYICI**
    *   **Gerekçe:** Şu anki en kritik hata budur. Kullanıcının sesi platforma temiz bir şekilde ulaşmadan, STT veya AI'ın çalışması imkansızdır. Ses kaydındaki cızırtı, gelen RTP paketlerinin `decode_g711_to_lpcm16` fonksiyonunda veya `inbound_samples` tamponuna yazılırken bozulduğunu gösteriyor. Bu hata, tüm diyalog akışını işlevsiz kılıyor.
    *   **Kabul Kriterleri:**
        1.  `end_to_end_call_validator` testi veya manuel bir test çağrısı sonucunda oluşturulan `.wav` kaydı indirildiğinde, hem kullanıcının sesi (inbound) hem de sistemin sesi (outbound) net, cızırtısız ve anlaşılır olmalıdır.
        2.  `stt-service` logları, gelen ses akışından anlamlı ve doğru bir transkript üretebildiğini göstermelidir.
        3.  `live_audio_client` test örneği çalıştırıldığında, alınan ses verisinin checksum'ı gönderilenle tutarlı olmalıdır.

*   **Görev ID:** `MEDIA-REFACTOR-02`
    *   **Başlık:** refactor(session): Anonsların kesilmesini önlemek için komut kuyruğu mekanizması ekle
    *   **Durum:** `[ ⬜ ] Yapılacak`
    *   **Öncelik:** **YÜKSEK**
    *   **Gerekçe:** Test çağrısında, giriş anonsu (`connecting.wav`), ilk TTS yanıtı tarafından kesilmiştir. `rtp_session_handler`, `PlayAudio` komutlarını sıraya koymak yerine bir önceki komutu iptal ediyor. Bu, kötü bir kullanıcı deneyimi yaratır. Her `rtp_session_handler` içinde, gelen `PlayAudio` komutlarını bir kuyruğa (queue) ekleyen ve bir önceki tamamlandığında sıradakini başlatan bir mekanizma olmalıdır.
    *   **Kabul Kriterleri:**
        1.  Bir çağrı başladığında, önce `connecting.wav` anonsu tamamen çalmalı, **bittikten sonra** ilk TTS yanıtı çalmaya başlamalıdır.
        2.  Ses kaydında her iki ses de tam ve kesintisiz olarak duyulmalıdır.

---

### **FAZ 1: Temel Medya Yetenekleri (Tamamlandı)**

-   [x] **Görev ID: MEDIA-CORE-01 - Port Yönetimi**
-   [x] **Görev ID: MEDIA-CORE-02 - Ses Çalma (`PlayAudio`)**
-   [x] **Görev ID: MEDIA-CORE-03 - Canlı Ses Akışı (`RecordAudio`)**
-   [x] **Görev ID: MEDIA-CORE-04 - Kalıcı Kayıt Altyapısı**
-   [x] **Görev ID: MEDIA-FEAT-03 - RabbitMQ Entegrasyonu**
-   [x] **Görev ID: MEDIA-004 - Zenginleştirilmiş Olay Yayınlama**

---

### **FAZ 2: Çift Yönlü Ses Kararlılığı ve Mimari Sağlamlaştırma (Tamamlandı)**

**Amaç:** Canlı testlerde tespit edilen kritik hataları çözmek, servisi platform standartlarına uygun, dayanıklı ve güvenli bir mimariye kavuşturmak.

-   [x] **Görev ID: MEDIA-BUG-01 - Tek Yönlü Ses ve Bozuk Kayıt Sorununu Giderme**
-   [x] **Görev ID: MEDIA-REFACTOR-01 - Dayanıklı Başlatma ve Graceful Shutdown**
-   [x] **Görev ID: MEDIA-IMPRV-01 - Dockerfile Güvenlik ve Standardizasyonu**

---

### **FAZ 3: Gelişmiş Medya Özellikleri (Gelecek Vizyonu)**

-   **Görev ID: MEDIA-FEAT-01 - Codec Müzakeresi**
    -   **Durum:** ⬜ **Planlandı**
    -   **Açıklama:** Gelen SDP'ye (Session Description Protocol) göre G.729 gibi farklı, daha verimli kodekleri destekleyerek bant genişliği kullanımını optimize etmek.

-   **Görev ID: MEDIA-FEAT-02 - Güvenli Medya (SRTP)**
    -   **Durum:** ⬜ **Planlandı**
    -   **Açıklama:** Medya akışını (RTP paketlerini) uçtan uca şifrelemek için SRTP (Secure Real-time Transport Protocol) desteği eklemek. Bu, en üst düzeyde güvenlik ve gizlilik gerektiren senaryolar için kritiktir.

-   **Görev ID: MEDIA-FEAT-04 - Anlık Ses Analizi**
    -   **Durum:** ⬜ **Planlandı**
    -   **Açıklama:** Ses akışı üzerinden anlık olarak duygu analizi (kullanıcının ses tonundan sinirli, mutlu vb. olduğunu anlama) veya anahtar kelime tespiti ("yöneticiye bağla" gibi) yapabilen bir altyapı kurmak. Bu, diyaloğu proaktif olarak yönlendirme imkanı sağlar.
    