# 🎙️ Sentiric Media Service

**Açıklama:** Bu servis, Sentiric platformundaki tüm gerçek zamanlı medya akışı (RTP) yönetiminden sorumludur. Yüksek performans, bellek güvenliği ve düşük seviye ağ kontrolü için **Rust** ile yazılmıştır.

**Temel Sorumluluklar:**
*   **Dinamik Port Yönetimi:** Diğer servislerin (örn: `sip-signaling`) RTP trafiği için dinamik olarak UDP portu talep etmesi ve serbest bırakması için bir gRPC arayüzü sağlar (`AllocatePort`, `ReleasePort`).
*   **Medya Oynatma:** `agent-service` gibi servislerden gelen komutlarla (`PlayAudio`), belirtilen bir hedefe (kullanıcının telefonu) önceden kaydedilmiş ses dosyalarını (anonslar, müzik vb.) RTP akışı olarak gönderir.
*   **RTP Akış Yönetimi:** Tahsis edilen portları dinler, gelen RTP trafiğini kabul eder ve her çağrı için ayrı bir oturum yönetir.

**Teknoloji Yığını:**
*   **Dil:** Rust
*   **Asenkron Runtime:** Tokio
*   **Servisler Arası İletişim:** gRPC (Tonic ile)

## Yerel Geliştirme ve Test

Bu servis, platformdan bağımsız olarak da test edilebilir.

### Önkoşullar
- Rust (rustup ile)
- Docker & Docker Compose (opsiyonel, konteynerli test için)

### Adım 1: Bağımlılıkları Kur
```bash
cargo build
```
Bu komut, `Cargo.lock` dosyasını oluşturacak ve gerekli tüm kütüphaneleri indirecektir.

### Adım 2: Yerel Test
1.  Repo ana dizininde bir `.env` dosyası oluşturun (`.env.example`'dan kopyalayarak).
2.  Repo ana dizininde `assets/audio/tr` şeklinde bir klasör yapısı oluşturun ve test için `.wav` dosyalarınızı (8000 Hz, mono, 16-bit PCM formatında) içine koyun.
3.  Servisi doğrudan çalıştırın:
    ```bash
    cargo run
    ```
    Servis `50052` portunda gRPC isteklerini dinlemeye başlayacaktır.

### Adım 3: Konteynerli Test
1.  Adım 2'deki `.env` ve `assets` klasörlerinin hazır olduğundan emin olun.
2.  Servisi kendi `docker-compose.service.yml` dosyası ile ayağa kaldırın:
    ```bash
    docker compose -f docker-compose.service.yml up --build -d
    ```
3.  Logları kontrol edin:
    ```bash
    docker logs -f sentiric_media_service
    ```
---

## 🚀 Performans ve Kapasite Planlaması

Servis, yüksek eş zamanlılık ve düşük kaynak tüketimi hedeflenerek tasarlanmıştır. Yapılan stres testleri, aşağıdaki konfigürasyon önerilerini ortaya koymuştur.

### CPU & Bellek
Servisin ana performans darboğazı CPU'dur. Standart bir production ortamı için **0.5 vCPU** ve **256 MB Bellek** ideal bir başlangıç noktasıdır. Bu konfigürasyon, saniyede ~55 çağrılık patlamalı bir trafiği stabil bir şekilde yönetebilir.

### RTP Port Aralığı
Port çakışmalarını önlemek için, `docker-compose.yml` veya ortam değişkenlerinizde **250-300 portluk** bir aralık (`EXTERNAL_RTP_PORT_MIN=10000`, `EXTERNAL_RTP_PORT_MAX=10600` gibi) tanımlanması önerilir. Çok geniş aralıklar (1000+) Docker'ın başlatılmasında sorunlara yol açabilir.

Daha detaylı performans analizi ve test sonuçları için `PERFORMANCE.md` dosyasına bakınız.