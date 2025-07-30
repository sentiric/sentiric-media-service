# Sentiric Media Service - Performans Profili ve Kapasite Planlaması

Bu doküman, servisin performans karakteristiklerini, test sonuçlarını ve üretim ortamı için önerilen kaynak yapılandırmalarını özetlemektedir.

## Özet Performans Metrikleri

Yapılan stres testleri sonucunda, `debian:bookworm-slim` tabanlı Docker imajı üzerinde çalışan servis için aşağıdaki temel performans metrikleri elde edilmiştir:

| vCPU Limiti | Stabil Eş Zamanlı Çağrı Kapasitesi (Yaklaşık) | Saniyedeki Ortalama Çağrı (CPS) |
|-------------|-----------------------------------------------|----------------------------------|
| **0.5 vCPU**| ~120-150                                      | ~55 CPS                          |
| **1.0 vCPU**| ~250-300+                                     | ~100+ CPS                        |

*Not: Bu değerler, ortalama 4-5 saniyelik anons süreleri ve patlamalı (bursty) trafik desenleri altında elde edilmiştir. Gerçek dünyadaki performans, ağ koşullarına ve anonsların karmaşıklığına göre değişebilir.*

---

## Kaynak Yapılandırma Önerileri

### 1. CPU ve Bellek

- **Darboğaz (Bottleneck):** Servisin ana performans sınırlayıcısı **CPU**'dur. Bellek kullanımı, önbelleğe alınan anons sayısına bağlı olmasına rağmen oldukça düşüktür (~50 MB).
- **Minimum Öneri:** Düşük trafikli ortamlar için en az `cpus: 0.25` ve `mem_limit: 128m` önerilir.
- **Standart Production Önerisi:** Orta yoğunluktaki bir trafik için **`cpus: 0.5`** ve **`mem_limit: 256m`** ideal bir başlangıç noktasıdır.
- **Yüksek Kapasite:** Yüksek çağrı hacimleri için, CPU kaynağı (`cpus` değeri) doğrusal olarak artırılabilir.

### 2. RTP Port Aralığı

Servis, yüksek eş zamanlılık altında port çakışmalarını önlemek için bir "soğuma süresi" (karantina) mekanizması kullanır. Bu nedenle, port havuzunun anlık aktif çağrı sayısından daha büyük olması kritik öneme sahiptir.

- **Problem:** Çok geniş bir port aralığı (örn: 1000+) Docker veya ana işletim sisteminin ağ yığınını zorlayabilir ve başlatma sorunlarına yol açabilir.
- **Hesaplama Formülü:** `Gereken Port Sayısı ≈ (Maksimum Aktif Çağrı) + (Saniyedeki Çağrı * Karantina Süresi)`
- **Öneri:**
  - Standart bir `0.5 vCPU` konfigürasyonu için, **250-300 portluk** bir aralık (`10000-10600` gibi) hem güvenli hem de stabildir.
  - Bu, `~150` aktif çağrıyı ve karantinadaki portları rahatlıkla yönetebilir.
  - Kapasite artırıldığında bu aralık da orantılı olarak genişletilmelidir.

---

## Test Metodolojisi

Performans testleri, projenin `sentiric-cli` reposunda bulunan `realistic_test_call.py` script'i ile gerçekleştirilmiştir. Bu script, değişken çağrı süreleri ve Poisson dağılımına uygun patlamalı trafik üreterek gerçek dünya senaryolarını simüle eder.

Detaylı test sonuçları ve senaryolar için projenin `docs/realistic_test.md` dosyasına bakınız.