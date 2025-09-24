# 🎙️ Sentiric Media Service

[![Status](https://img.shields.io/badge/status-active-success.svg)]()
[![Language](https://img.shields.io/badge/language-Rust-orange.svg)]()
[![Protocol](https://img.shields.io/badge/protocol-gRPC_(mTLS)_&_RTP-green.svg)]()

**Sentiric Media Service**, Sentiric platformundaki tüm gerçek zamanlı medya akışı (RTP) yönetiminden sorumludur. Yüksek performans, bellek güvenliği ve düşük seviye ağ kontrolü için **Rust** ile yazılmıştır. Servis, bağımlılıkları hazır olana kadar bekleyen **dayanıklı bir başlatma** mekanizmasına ve güvenli, **root olmayan bir kullanıcı** ile çalışma prensibine sahiptir.

## 🎯 Temel Sorumluklar

*   **Dinamik Port Yönetimi:** Diğer servislerin RTP trafiği için dinamik olarak UDP portu talep etmesi (`AllocatePort`) ve serbest bırakması (`ReleasePort`) için bir gRPC arayüzü sağlar.
*   **Çift Yönlü Medya İşleme:**
    *   **Medya Oynatma:** `agent-service`'ten gelen komutlarla (`PlayAudio`), ses dosyalarını veya anlık TTS verisini kullanıcıya RTP akışı olarak gönderir.
    *   **Canlı Ses Akışı:** Kullanıcıdan gelen RTP sesini alır, standart formata dönüştürür ve `stt-service` tarafından işlenmesi için `agent-service`'e canlı olarak stream eder.
*   **Çağrı Kaydı ve Birleştirme:** Konuşmanın her iki tarafını da (kullanıcı ve bot) yakalar, tek bir mono ses kanalında birleştirir ve kalıcı depolama için S3 uyumlu bir hedefe yazar.
*   **Kaynak Yönetimi:** Port çakışmalarını önlemek için bir "port karantinası" mekanizması ve performansı artırmak için bir "ses önbelleği" kullanır.

## 🛠️ Teknoloji Yığını

*   **Dil:** Rust
*   **Asenkron Runtime:** Tokio
*   **Servisler Arası İletişim:** gRPC (Tonic ile, mTLS destekli)
*   **Medya Protokolü:** RTP (PCMU/PCMA kodek desteği)
*   **Kalıcı Depolama:** S3 Uyumlu Object Storage (örn: MinIO, AWS S3)
*   **Olay Yayınlama:** RabbitMQ
*   **Gözlemlenebilirlik:** `tracing` ile yapılandırılmış, standart (UTC, RFC3339) loglama ve Prometheus metrikleri.

## 🔌 API Etkileşimleri

*   **Gelen (Sunucu):**
    *   `sentiric-agent-service` (gRPC): Anons çalmak, canlı dinleme ve kayıt başlatmak/durdurmak için.
    *   *Not: `sip-signaling-service` ile olan doğrudan bağımlılık kaldırılmıştır, port yönetimi artık dolaylı olarak agent tarafından tetiklenir.*
*   **Giden (İstemci):**
    *   `S3/MinIO`: Çağrı kayıtlarını yazmak için.
    *   `RabbitMQ`: Kayıt tamamlandığında `call.recording.available` olayını yayınlamak için.
    *   *Kullanıcıya giden RTP paketleri.*

## 🚀 Yerel Geliştirme

1.  **Bağımlılıkları Yükleyin:**
2.  **Ortam Değişkenlerini Ayarlayın:** `.env.example` dosyasını `.env` olarak kopyalayın ve gerekli değişkenleri doldurun.
3.  **Servisi Çalıştırın:**

## 🤝 Katkıda Bulunma

Katkılarınızı bekliyoruz! Lütfen projenin ana [Sentiric Governance](https://github.com/sentiric/sentiric-governance) reposundaki kodlama standartlarına ve katkıda bulunma rehberine göz atın.

---
## 🏛️ Anayasal Konum

Bu servis, [Sentiric Anayasası'nın (v11.0)](https://github.com/sentiric/sentiric-governance/blob/main/docs/blueprint/Architecture-Overview.md) **Telekom & Medya Katmanı**'nda yer alan temel bir bileşendir.
