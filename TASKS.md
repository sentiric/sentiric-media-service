# 🎙️ Sentiric Media Service - Geliştirme Yol Haritası (v5.5 - Çift Yönlü Ses)

Bu belge, media-service'in geliştirme yol haritasını, tamamlanan görevleri ve mevcut öncelikleri tanımlar.

---

### **FAZ 1: Temel Medya Yetenekleri (Mevcut Durum)**

**Amaç:** Platformun temel medya işlevlerini (port yönetimi, ses çalma, kayıt) sağlayan altyapıyı kurmak.

-   [x] **Görev ID: MEDIA-CORE-01 - Port Yönetimi:** `AllocatePort` ve `ReleasePort` RPC'leri ile dinamik RTP portu tahsisi ve karantina mekanizması.
-   [x] **Görev ID: MEDIA-CORE-02 - Ses Çalma (`PlayAudio`):** `file://` (önceden kaydedilmiş) ve `data:` (TTS'ten gelen) URI'lerini destekleyen ses çalma yeteneği.
-   [x] **Görev ID: MEDIA-CORE-03 - Canlı Ses Akışı (`RecordAudio`):** Gelen RTP sesini gRPC üzerinden canlı olarak stream etme yeteneği.
-   [x] **Görev ID: MEDIA-CORE-04 - Kalıcı Kayıt Altyapısı:** `Start/StopRecording` RPC'leri ve S3/MinIO'ya kayıt yazma altyapısı.
-   [x] **Görev ID: MEDIA-FEAT-03 - RabbitMQ Entegrasyonu:** Kayıt tamamlandığında olay yayınlama yeteneği.
-   [x] **Görev ID: MEDIA-004 - Zenginleştirilmiş Olay Yayınlama:** `call.recording.available` olayına `call_id` ve `trace_id` ekleyerek `cdr-service` ile entegrasyonu sağlama.

---

### **FAZ 2: Çift Yönlü Ses Kararlılığı ve Bütünlüğü (Mevcut Odak)**

**Amaç:** Canlı testlerde tespit edilen kritik tek yönlü ses ve bozuk kayıt sorunlarını çözerek, platformun en temel gereksinimi olan çift yönlü, temiz ses iletişimini garanti altına almak.

-   **Görev ID: MEDIA-BUG-01 - Tek Yönlü Ses ve Bozuk Kayıt Sorununu Giderme (KRİTİK)**
    -   **Durum:** ⬜ **Yapılacak (ÖNCELİK 1)**
    -   **Problem Tanımı:** Canlı testler, son kullanıcıdan gelen sesin ne STT servisine doğru bir şekilde ulaştığını ne de kalıcı çağrı kaydına doğru kaydedildiğini göstermiştir. Kayıtlarda sadece sistemin (TTS) sesi duyulmakta, kullanıcının sesi ise ya hiç duyulmamakta ya da bozuk bir 'cızırtı' olarak yer almaktadır. Bu durum, `stt-service`'in hatalı transkripsiyon yapmasına ve `agent-service`'in diyalog döngüsünü kırmasına neden olan temel sorundur.
    -   **Kök Neden Analizi:** Sorunun, `media-service`'in gelen RTP paketlerini çözme (decode), sistemin iç standardı olan 16kHz LPCM formatına yeniden örnekleme (resample) veya bu sesi giden sesle birleştirme (mix) aşamasındaki bir hatadan kaynaklandığı kuvvetle muhtemeldir.
    -   **Çözüm Stratejisi:**
        1.  **Girdi Doğrulama:** Gelen RTP paketlerinin `payload`'ları ham olarak loglanmalı ve beklenen formatta (örn: G.711 PCMU) olduğu doğrulanmalıdır.
        2.  **Birim Testleri:** `rtp/codecs.rs` modülündeki G.711 -> LPCM dönüşüm ve `rubato` ile yapılan yeniden örnekleme mantığı için, bilinen bir girdi ve beklenen bir çıktı ile birim testleri yazılmalıdır. `sentiric-sip-core-service` projesindeki başarılı codec işleme mantığı referans alınabilir.
        3.  **Birleştirme Mantığı İncelemesi:** Gelen (kullanıcı) ve giden (TTS) ses kanallarını kalıcı kayıt için birleştiren mantık, bir kanalın diğerini ezmediğinden veya bozmadığından emin olmak için dikkatle incelenmelidir.
    -   **Kabul Kriterleri:**
        -   [ ] Bir test araması sonrası S3'e kaydedilen `.wav` dosyasında, **hem kullanıcının sesi hem de sistemin sesi** net ve anlaşılır bir şekilde duyulmalıdır.
        -   [ ] `stt-service`, test araması sırasında kullanıcının konuştuğu anlamlı cümleleri doğru bir şekilde metne çevirebilmelidir.
        -   [ ] `agent-service`, kullanıcının konuşmasını anladığı için sürekli olarak `ANNOUNCE_SYSTEM_CANT_UNDERSTAND` veya `ANNOUNCE_SYSTEM_CANT_HEAR_YOU` anonslarını tetiklememelidir.

---

### **FAZ 3: Gelişmiş Medya Özellikleri (Gelecek Vizyonu)**

-   [ ] **Görev ID: MEDIA-FEAT-01 - Codec Müzakeresi:** Gelen SDP'ye göre G.729 gibi farklı kodekleri destekleme.
-   [ ] **Görev ID: MEDIA-FEAT-02 - Güvenli Medya (SRTP):** Ses akışını uçtan uca şifrelemek için SRTP desteği ekleme.
-   [ ] **Görev ID: MEDIA-FEAT-04 - Anlık Ses Analizi:** Ses akışı üzerinden anlık olarak duygu analizi veya anahtar kelime tespiti yapabilme.