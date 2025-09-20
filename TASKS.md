# ========== DOSYA: sentiric-media-service/TASKS.md (TAM VE GÜNCEL İÇERİK) ==========
# 🎙️ Sentiric Media Service - Geliştirme Yol Haritası (v8.1 - Doğal Akış Odaklı)

Bu belge, media-service'in geliştirme yol haritasını, tamamlanan görevleri ve mevcut öncelikleri tanımlar.

---

### **FAZ 3: MİMARİ SAĞLAMLAŞTIRMA (Sıradaki Öncelik)**

**Amaç:** Servisin temel medya işleme mantığını, doğal diyalog akışını ("barge-in" / söz kesme) ve doğru kayıt birleştirmeyi destekleyecek şekilde yeniden yapılandırmak.

-   **Görev ID: MEDIA-REFACTOR-01 - Gerçek Zamanlı Ses Birleştirme (Real-time Audio Mixing)**
    -   **Durum:** ⬜ **Yapılacak (Öncelik 1)**
    -   **Problem:** Mevcut kayıt mekanizması, gelen (inbound) ve giden (outbound) ses kanallarını ayrı ayrı biriktirip çağrı sonunda birleştiriyor. Bu, ses kayıtlarının doğal olmayan bir şekilde "önce bot konuşur, sonra kullanıcı konuşur" şeklinde olmasına neden oluyor.
    -   **Açıklama:** `rtp_session_handler`'ın mantığı, ayrı `inbound_samples` ve `outbound_samples` listeleri tutmak yerine, gelen ve giden tüm ses örneklerini (LPCM) **tek bir zaman damgalı (timestamped) olay kuyruğuna veya doğrudan birleşik bir `mixed_samples` tamponuna** anlık olarak yazacak şekilde yeniden tasarlanmalıdır.
    -   **Kabul Kriterleri:**
        -   [ ] `rtp_session.rs` içindeki `RecordingSession` yapısı, iki ayrı `Vec<i16>` yerine, birleşik ses örneklerini tutacak yeni bir yapı kullanmalıdır.
        -   [ ] `rtp_session_handler`, hem gelen RTP paketlerinden çözülen hem de `PlayAudio` komutuyla çalınan ses örneklerini anlık olarak bu yeni birleşik tampona yazmalıdır.
        -   [ ] `StopPermanentRecording` komutu geldiğinde, bu önceden birleştirilmiş tampon doğrudan WAV formatına çevrilip S3'e yazılmalıdır.
        -   [ ] **Doğrulama:** Test çağrısı kaydı dinlendiğinde, kullanıcı ve botun konuşmaları, gerçek bir diyalogdaki gibi iç içe geçmiş ve doğru zamanlamayla duyulmalıdır.

---

### **FAZ 4: Gelişmiş Hata Yönetimi ve Dayanıklılık (Planlandı)**

-   **Görev ID: MEDIA-FEAT-01 - Codec Müzakeresi**
    -   **Durum:** ⬜ **Planlandı**
-   **Görev ID: MEDIA-FEAT-02 - Güvenli Medya (SRTP)**
    -   **Durum:** ⬜ **Planlandı**
-   **Görev ID: MEDIA-FEAT-04 - Anlık Ses Analizi**
    -   **Durum:** ⬜ **Planlandı**