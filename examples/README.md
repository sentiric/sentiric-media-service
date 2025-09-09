# ⚡️ Media Service - Test ve Simülasyon İstemcileri

Bu dizin, `media-service`'in çeşitli fonksiyonlarını test etmek, simüle etmek ve doğrulamak için kullanılan örnek istemci uygulamalarını içerir.

Testler iki ana kategoriye ayrılmıştır:
1.  **Hızlı Yerel Testler:** Belirli bir özelliği anında, yerel makinenizde denemek için kullanılır.
2.  **Tam Otomatize Entegrasyon Testleri:** Projenin bütünsel sağlığını kontrol eden, CI/CD için tasarlanmış tam kapsamlı testlerdir.

---

## 1. Hızlı Yerel Testler (`cargo run --example`)

Bu komutlar, `development.env` dosyasındaki yapılandırmaları kullanarak yerel makinenizde çalışır. Docker gerektirmezler ancak `media-service`'in başka bir terminalde `cargo run --release` ile çalışıyor olması gerekir.

### Temel Bağlantı ve API Testleri

-   **Agent Service gibi davran (Port Al/Bırak):**
    ```bash
    cargo run --example agent_client --release
    ```
-   **Dialplan gibi davran (Anons Çal):**
    ```bash
    cargo run --example dialplan_client --release
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

### En Kapsamlı Manuel ve Otomatik Testler

-   **Uçtan Uca Temel Doğrulama (CI için de kullanılır):**
    > ⚠️ **Not:** Bu test, `.env.development` veya `.env.test` dosyasında doğru IP adreslerinin ayarlanmasını gerektirir.
    ```bash
    cargo run --example end_to_end_call_validator --release
    ```
-   **✨ ALTIN STANDART: Gerçekçi Çağrı Akışı Simülasyonu:**
    > Bu test, cızırtı ve anons kesilmesi sorunlarının çözümünü kanıtlamak için tasarlanmıştır. Gerçek bir çağrıdaki gibi sıralı anonsları, anlık TTS yanıtını ve eş zamanlı kullanıcı konuşmasını simüle eder. **Projedeki en önemli ve kapsamlı testtir.**
    ```bash
    cargo run --example realistic_call_flow --release
    ```

---

## 2. Tam Otomatize Entegrasyon Testleri (Docker)

Bu komut, projenin tamamını (MinIO dahil) Docker içinde ayağa kaldırır, en kritik senaryo olan `end_to_end_call_validator`'ı otomatize bir şekilde çalıştırır ve sonucunu bildirir.

**CI/CD ve son doğrulama için kullanılması gereken yöntem budur.**

-   **Tüm Entegrasyon Testlerini Docker Ortamında Çalıştır:**
    ```bash
    docker-compose -f docker-compose.test.yml up --build --exit-code-from test-runner
    ```
    -   `--build`: Testleri çalıştırmadan önce imajları yeniden oluşturur.
    -   `--exit-code-from test-runner`: Testler bittiğinde `test-runner` container'ının çıkış kodunu (0: başarılı, >0: başarısız) döndürür ve tüm ortamı otomatik olarak kapatır.

--env-file .env.test: IP adreslerini ve diğer test ayarlarını yükler. Bu en kritik kısım.

--abort-on-container-exit: Testler bittiğinde veya herhangi bir konteyner hata verip kapandığında, tüm ortamı otomatik olarak temizler. Bu, --exit-code-from'a göre daha modern bir alternatiftir.    


    ```bash
    docker-compose --env-file .env.test -f docker-compose.test.yml up --build --abort-on-container-exit
    ```