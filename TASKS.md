# 🎙️ Sentiric Media Service - Geliştirme Yol Haritası (v8.0 - Gözlemlenebilirlik ve Kararlılık)

Bu belge, media-service'in geliştirme yol haritasını, tamamlanan görevleri ve mevcut öncelikleri tanımlar.

---

### **FAZ 3: Üretim Ortamı Kararlılığı ve Gözlemlenebilirlik (Tamamlandı)**

**Amaç:** Platform genelinde yapılan uçtan uca testlerde tespit edilen kritik hataları çözmek, servisin loglama altyapısını production standartlarına yükseltmek ve farklı S3 uyumlu sağlayıcılarla (MinIO, Cloudflare R2) sorunsuz çalışmasını garanti altına almak.

*   **Görev ID:** `MEDIA-BUG-03`
    *   **Başlık:** fix(recording): S3/Cloudflare R2 Kayıt Yazma Hatasını Gider
    *   **Durum:** `[ ✅ ] Tamamlandı`
    *   **Öncelik:** **KRİTİK**
    *   **Çözüm:** `aws-sdk-s3` kütüphanesinden dönen hataların detaylı loglanması sağlandı. Kök nedenin, test istemcisi ile sunucu arasındaki S3 anahtar (key) yolu oluşturma mantığındaki tutarsızlıktan (baştaki `/` karakteri) kaynaklandığı tespit edildi. Hem `end_to_end_call_validator` hem de `realistic_call_flow` testlerindeki URI oluşturma mantığı, sunucunun `writers.rs` modülüyle uyumlu hale getirilerek sorun kalıcı olarak çözüldü. Testler artık hem lokal MinIO hem de harici Cloudflare R2 sağlayıcılarında başarıyla çalışmaktadır.

*   **Görev ID:** `MEDIA-IMPRV-02`
    *   **Başlık:** perf(logging): INFO Seviyesindeki Log Gürültüsünü Azalt ve Yapısal Hale Getir
    *   **Durum:** `[ ✅ ] Tamamlandı`
    *   **Öncelik:** **ORTA**
    *   **Çözüm:** Loglama altyapısı, tüm ortamlarda yapısal (JSON) log üretecek şekilde güncellendi. `data:` URI'si içeren `PlayAudio` isteklerinin loglanması, base64 içeriğini yazdırmayacak, bunun yerine URI şeması, boyutu ve kısa bir önizleme içerecek şekilde akıllı hale getirildi. `RUST_LOG` ortam değişkeni, `symphonia`, `aws` gibi harici kütüphanelerden gelen gürültüyü `WARN` seviyesine indirgeyecek şekilde ayarlandı. Gereksiz `INFO` seviyesi loglar `DEBUG` seviyesine çekilerek logların okunabilirliği ve anlamlılığı artırıldı.

---

### **FAZ 2: Çift Yönlü Ses Kararlılığı ve Mimari Sağlamlaştırma (Tamamlandı)**

*   **Görev ID:** `MEDIA-BUG-02` - fix(rtp): Gelen RTP (inbound) ses akışındaki bozulmayı ve cızırtıyı düzelt `[ ✅ ]`
*   **Görev ID:** `MEDIA-REFACTOR-02` - refactor(session): Anonsların kesilmesini önlemek için komut kuyruğu mekanizması ekle `[ ✅ ]`
*   **Görev ID:** `MEDIA-BUG-01` - Tek Yönlü Ses ve Bozuk Kayıt Sorununu Giderme `[ ✅ ]`
*   **Görev ID:** `MEDIA-REFACTOR-01` - Dayanıklı Başlatma ve Graceful Shutdown `[ ✅ ]`
*   **Görev ID:** `MEDIA-IMPRV-01` - Dockerfile Güvenlik ve Standardizasyonu `[ ✅ ]`

---

### **FAZ 1: Temel Medya Yetenekleri (Tamamlandı)**

*   [x] **Görev ID: MEDIA-CORE-01 - Port Yönetimi**
*   [x] **Görev ID: MEDIA-CORE-02 - Ses Çalma (`PlayAudio`)**
*   [x] **Görev ID: MEDIA-CORE-03 - Canlı Ses Akışı (`RecordAudio`)**
*   [x] **Görev ID: MEDIA-CORE-04 - Kalıcı Kayıt Altyapısı**
*   [x] **Görev ID: MEDIA-FEAT-03 - RabbitMQ Entegrasyonu**
*   [x] **Görev ID: MEDIA-004 - Zenginleştirilmiş Olay Yayınlama**

---

### **FAZ 4: Gelişmiş Hata Yönetimi ve Dayanıklılık (Sıradaki Öncelik)**

**Amaç:** Servisin, bağımlı olduğu altyapılardaki (özellikle S3) hataları daha anlamlı bir şekilde yakalayıp, kendisini çağıran servislere (agent-service) daha net bilgi vermesini sağlamak.

-   **Görev ID: MEDIA-IMPRV-03 - feat(error-handling): S3 Hatalarını Daha Detaylı gRPC Durumlarına Eşle**
    -   **Durum:** ⬜ **Yapılacak (Öncelik 1 - YÜKSEK)**
    -   **Açıklama:** Şu anda `StopRecording` başarısız olduğunda, `agent-service`'e genel bir "Internal Error" dönülüyor. Bu durum, `agent-service`'in hatanın nedenini anlamasını engelliyor. `NoSuchBucket` gibi spesifik S3 hataları, daha anlamlı gRPC durum kodlarına (örn: `FailedPrecondition`) ve mesajlarına dönüştürülmelidir.
    -   **Kabul Kriterleri:**
        -   [ ] `rtp/session_utils.rs` içindeki `finalize_and_save_recording` fonksiyonunda, `writers::from_uri` veya `writer.write`'tan dönen hatalar detaylı olarak incelenmelidir.
        -   [ ] Eğer hata metni "NoSuchBucket" içeriyorsa, `tonic::Status::failed_precondition("Kayıt hedefi (S3 Bucket) bulunamadı veya yapılandırılmamış.")` gibi bir hata döndürülmelidir.
        -   [ ] Eğer hata "Access Denied" içeriyorsa, `tonic::Status::permission_denied("S3 Bucket'ına yazma izni yok.")` gibi bir hata döndürülmelidir.
        -   [ ] Bu değişiklik sonrası, altyapıdaki `INFRA-FIX-01` görevi yapılmadan test edildiğinde, `agent-service` loglarında artık "Internal Error" yerine "FailedPrecondition: Kayıt hedefi bulunamadı..." gibi anlamlı bir hata görülmelidir.

-   **Görev ID: MEDIA-FEAT-01 - Codec Müzakeresi**
    -   **Durum:** ⬜ **Planlandı**
    -   **Açıklama:** Gelen SDP'ye (Session Description Protocol) göre G.729 gibi farklı, daha verimli kodekleri destekleyerek bant genişliği kullanımını optimize etmek.

-   **Görev ID: MEDIA-FEAT-02 - Güvenli Medya (SRTP)**
    -   **Durum:** ⬜ **Planlandı**
    -   **Açıklama:** Medya akışını (RTP paketlerini) uçtan uca şifrelemek için SRTP (Secure Real-time Transport Protocol) desteği eklemek. Bu, en üst düzeyde güvenlik ve gizlilik gerektiren senaryolar için kritiktir.

-   **Görev ID: MEDIA-FEAT-04 - Anlık Ses Analizi**
    -   **Durum:** ⬜ **Planlandı**
    -   **Açıklama:** Ses akışı üzerinden anlık olarak duygu analizi (kullanıcının ses tonundan sinirli, mutlu vb. olduğunu anlama) veya anahtar kelime tespiti ("yöneticiye bağla" gibi) yapabilen bir altyapı kurmak. Bu, diyaloğu proaktif olarak yönlendirme imkanı sağlar.