# 🎙️ Sentiric Media Service - Geliştirme Yol Haritası (v8.0 - Gözlemlenebilirlik ve Kararlılık)

Bu belge, media-service'in geliştirme yol haritasını, tamamlanan görevleri ve mevcut öncelikleri tanımlar.

---

### **FAZ 4: Gelişmiş Hata Yönetimi ve Dayanıklılık (Sıradaki Öncelik)**

**Amaç:** Servisin, bağımlı olduğu altyapılardaki (özellikle S3) hataları daha anlamlı bir şekilde yakalayıp, kendisini çağıran servislere (agent-service) daha net bilgi vermesini sağlamak.


-   **Görev ID: MEDIA-FEAT-01 - Codec Müzakeresi**
    -   **Durum:** ⬜ **Planlandı**
    -   **Açıklama:** Gelen SDP'ye (Session Description Protocol) göre G.729 gibi farklı, daha verimli kodekleri destekleyerek bant genişliği kullanımını optimize etmek.

-   **Görev ID: MEDIA-FEAT-02 - Güvenli Medya (SRTP)**
    -   **Durum:** ⬜ **Planlandı**
    -   **Açıklama:** Medya akışını (RTP paketlerini) uçtan uca şifrelemek için SRTP (Secure Real-time Transport Protocol) desteği eklemek. Bu, en üst düzeyde güvenlik ve gizlilik gerektiren senaryolar için kritiktir.

-   **Görev ID: MEDIA-FEAT-04 - Anlık Ses Analizi**
    -   **Durum:** ⬜ **Planlandı**
    -   **Açıklama:** Ses akışı üzerinden anlık olarak duygu analizi (kullanıcının ses tonundan sinirli, mutlu vb. olduğunu anlama) veya anahtar kelime tespiti ("yöneticiye bağla" gibi) yapabilen bir altyapı kurmak. Bu, diyaloğu proaktif olarak yönlendirme imkanı sağlar.