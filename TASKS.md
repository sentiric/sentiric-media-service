# 🎙️ Sentiric Media Service - Görev Listesi

Bu belge, `media-service`'in geliştirme yol haritasını ve önceliklerini tanımlar.

---

### Faz 1: Temel Medya Akışı (Mevcut Durum)

Bu faz, servisin temel port yönetimi ve medya oynatma görevlerini yerine getirmesini hedefler.

-   [x] **gRPC Sunucusu:** `AllocatePort`, `ReleasePort` ve `PlayAudio` RPC'lerini implemente eden sunucu.
-   [x] **Dinamik Port Yönetimi:** Belirtilen aralıktan UDP portlarını dinamik olarak tahsis etme.
-   [x] **Port Karantinası:** Kullanılan portları bir süre bekletip tekrar havuza ekleyen arka plan görevi.
-   [x] **RTP Oynatma:** `.wav` dosyalarını okuyup, μ-law formatına çevirip RTP paketleri olarak gönderme.
-   [x] **Ses Önbellekleme:** Sık kullanılan ses dosyalarını hafızada tutma.

---

### Faz 2: Gelişmiş Medya ve Güvenlik (Sıradaki Öncelik)

Bu faz, servise kayıt ve güvenlik gibi kritik yetenekler eklemeyi hedefler.

-   [ ] **Görev ID: MEDIA-001 - Ses Kaydı (`RecordAudio`)**
    -   **Açıklama:** Gelen RTP akışını alıp bir `.wav` dosyasına kaydeden yeni bir gRPC endpoint'i (`RecordAudio`) ekle. Bu, STT işlemleri ve yasal kayıt gereksinimleri için temel oluşturacaktır.
    -   **Durum:** ⬜ Planlandı.

-   [ ] **Görev ID: MEDIA-002 - Güvenli RTP (SRTP) Desteği**
    -   **Açıklama:** Medya akışını şifrelemek için SRTP protokolünü implemente et. Anahtar değişimi için DTLS-SRTP veya ZRTP yöntemlerini araştır.
    -   **Durum:** ⬜ Planlandı.

---

### Faz 3: Konferans ve Transcoding

Bu faz, servisi çoklu katılımcılı senaryoları destekleyen bir medya sunucusuna dönüştürmeyi hedefler.

-   [ ] **Görev ID: MEDIA-003 - Konferans Köprüsü (Conference Bridge)**
    -   **Açıklama:** Birden fazla RTP akışını tek bir odada birleştirip tüm katılımcılara geri gönderen bir konferans köprüsü özelliği ekle.
    *   **Durum:** ⬜ Planlandı.

-   [ ] **Görev ID: MEDIA-004 - Codec Transcoding**
    -   **Açıklama:** Farklı codec'ler (örn: G.711, G.729, Opus) arasında gerçek zamanlı dönüşüm yapma yeteneği ekle.
    *   **Durum:** ⬜ Planlandı.