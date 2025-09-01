# ⚡️ Media Service - Test ve Simülasyon İstemcileri

Bu dizin, `media-service`'in çeşitli fonksiyonlarını test etmek, simüle etmek ve doğrulamak için kullanılan örnek istemci uygulamalarını içerir.

Testler iki ana kategoriye ayrılmıştır:
1.  **Hızlı Yerel Testler:** Belirli bir özelliği anında, yerel makinenizde denemek için kullanılır.
2.  **Tam Otomatize Entegrasyon Testleri:** Projenin bütünsel sağlığını kontrol eden, CI/CD için tasarlanmış tam kapsamlı testlerdir.

---

## 1. Hızlı Yerel Testler (`cargo run --example`)

Bu komutlar, `development.env` dosyasındaki yapılandırmaları kullanarak yerel makinenizde çalışır. Docker gerektirmezler ancak `media-service`'in başka bir terminalde `cargo run --release` ile çalışıyor olması gerekir.

### Temel Bağlantı ve API Testleri

-   **Tüm Ortam Değişkenlerini Doğrula:**
    ```bash
    cargo run --example test --release
    ```
-   **Agent Service gibi davran (Port Al/Bırak):**
    ```bash
    cargo run --example agent_client --release
    ```
-   **SIP Signaling gibi davran (Kapasite Kontrolü):**
    ```bash
    cargo run --example sip_signaling_client --release
    ```
-   **Dialplan gibi davran (Anons Çal):**
    ```bash
    cargo run --example dialplan_client --release
    ```
-   **User Service gibi davran (Sadece Bağlantı Testi):**
    ```bash
    cargo run --example user_client --release
    ```

### Fonksiyonel Senaryo Testleri

-   **Canlı Ses Akışını Test Et (STT Simülasyonu):**
    ```bash
    cargo run --example live_audio_client --release
    ```
-   **S3'e Kalıcı Çağrı Kaydını Test Et:**
    ```bash
    cargo run --example recording_client --release
    ```
-   **Performans ve Stres Testi Uygula:**
    ```bash
    cargo run --example call_simulator --release
    ```

### En Kapsamlı Manuel Test

-   **Uçtan Uca Diyalog ve Ses Birleştirmeyi Doğrula:**
    > ⚠️ **Not:** Bu test, `development.env` dosyasında doğru IP adreslerinin (`MEDIA_SERVICE_PUBLIC_IP`) ayarlanmasını gerektirir.
    ```bash
    cargo run --example end_to_end_call_validator --release
    ```

---

## 2. Tam Otomatize Entegrasyon Testleri (Docker)

Bu komut, projenin tamamını (MinIO dahil) Docker içinde ayağa kaldırır, tüm kritik senaryoları (`live_audio`, `recording`, `end_to_end_validator`) otomatize bir şekilde çalıştırır ve sonucunu bildirir.

**CI/CD ve son doğrulama için kullanılması gereken yöntem budur.**

-   **Tüm Entegrasyon Testlerini Docker Ortamında Çalıştır:**
    ```bash
    docker-compose -f docker-compose.test.yml up --build --exit-code-from test-runner
    ```
    -   `--build`: Testleri çalıştırmadan önce imajları yeniden oluşturur.
    -   `--exit-code-from test-runner`: Testler bittiğinde `test-runner` container'ının çıkış kodunu (0: başarılı, >0: başarısız) döndürür ve tüm ortamı otomatik olarak kapatır.
