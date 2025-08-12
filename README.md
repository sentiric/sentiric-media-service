# 🎙️ Sentiric Media Service

[![Status](https://img.shields.io/badge/status-active-success.svg)]()
[![Language](https://img.shields.io/badge/language-Rust-orange.svg)]()
[![Protocol](https://img.shields.io/badge/protocol-gRPC_(mTLS)_&_RTP-green.svg)]()

**Sentiric Media Service**, Sentiric platformundaki tüm gerçek zamanlı medya akışı (RTP) yönetiminden sorumludur. Yüksek performans, bellek güvenliği ve düşük seviye ağ kontrolü için **Rust** ile yazılmıştır.

## 🎯 Temel Sorumluluklar

*   **Dinamik Port Yönetimi:** Diğer servislerin (örn: `sip-signaling`) RTP trafiği için dinamik olarak UDP portu talep etmesi (`AllocatePort`) ve serbest bırakması (`ReleasePort`) için bir gRPC arayüzü sağlar.
*   **Medya Oynatma:** `agent-service` gibi servislerden gelen komutlarla (`PlayAudio`), belirtilen bir hedefe (kullanıcının telefonu) önceden kaydedilmiş ses dosyalarını RTP akışı olarak gönderir.
*   **Port Karantinası:** Kullanımdan sonra bir portun hemen yeniden kullanılmasını önleyen bir "soğuma süresi" mekanizması ile ağ kararlılığını artırır.
*   **Ses Önbellekleme:** Sık çalınan anonsları hafızada tutarak disk I/O'sunu azaltır ve yanıt süresini iyileştirir.

## 🛠️ Teknoloji Yığını

*   **Dil:** Rust
*   **Asenkron Runtime:** Tokio
*   **Servisler Arası İletişim:** gRPC (Tonic ile, mTLS destekli)
*   **Medya Protokolü:** RTP
*   **Gözlemlenebilirlik:** `tracing` ile yapılandırılmış, ortama duyarlı loglama.

## 🔌 API Etkileşimleri

*   **Gelen (Sunucu):**
    *   `sentiric-sip-signaling-service` (gRPC): Çağrı kurulumu sırasında port talep etmek için.
    *   `sentiric-agent-service` (gRPC): Anons çalmak gibi medya işlemleri için.
*   **Giden (İstemci):**
    *   Bu servis giden API çağrısı yapmaz. Sadece RTP paketleri gönderir.

## 🚀 Yerel Geliştirme

1.  **Bağımlılıkları Yükleyin:** `cargo build`
2.  **`.env` Dosyasını Oluşturun:** `sentiric-agent-service/.env.docker` dosyasını referans alarak gerekli sertifika yollarını ve port aralıklarını tanımlayın.
3.  **Servisi Çalıştırın:** `cargo run --release`

## 🤝 Katkıda Bulunma

Katkılarınızı bekliyoruz! Lütfen projenin ana [Sentiric Governance](https://github.com/sentiric/sentiric-governance) reposundaki kodlama standartlarına ve katkıda bulunma rehberine göz atın.

---
## 🏛️ Anayasal Konum

Bu servis, [Sentiric Anayasası'nın (v11.0)](https://github.com/sentiric/sentiric-governance/blob/main/docs/blueprint/Architecture-Overview.md) **Zeka & Orkestrasyon Katmanı**'nda yer alan merkezi bir bileşendir.