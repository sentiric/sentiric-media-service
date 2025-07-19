package main

import (
	"encoding/base64"
	"fmt"
	"log"
	"math/rand"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"sync"
	"time"

	"github.com/gofiber/fiber/v2"
	"github.com/google/uuid"
)

// --- Ayarlar ---
var (
	PUBLIC_IP    = getEnv("PUBLIC_IP", "127.0.0.1")
	API_PORT     = getEnv("PORT", "3003")
	RTP_PORT_MIN = getEnvAsInt("RTP_PORT_MIN", 10000)
	RTP_PORT_MAX = getEnvAsInt("RTP_PORT_MAX", 10100)
)

// --- Veri Yapıları ---
type RtpSession struct {
	Socket     *net.UDPConn
	RemoteInfo *net.UDPAddr
}

// Eşzamanlı erişimler için güvenli bir map yapısı
var rtpSessions = struct {
	sync.RWMutex
	m map[int]*RtpSession
}{m: make(map[int]*RtpSession)}

// --- Ana Fonksiyon ---
func main() {
	app := fiber.New()

	app.Get("/rtp-session", handleRtpSession)
	app.Post("/play-audio", handlePlayAudio)

	log.Printf("✅ [Media Service - GO] Servis http://0.0.0.0:%s adresinde dinlemede.", API_PORT)
	log.Printf("[Media Service - GO] RTP port aralığı: %d-%d", RTP_PORT_MIN, RTP_PORT_MAX)
	log.Fatal(app.Listen(":" + API_PORT))
}

// --- Endpoint Handler'ları ---
func handleRtpSession(c *fiber.Ctx) error {
	log.Println("[Media Service - GO] Yeni RTP oturumu için istek alındı.")
	port, conn, err := allocateRtpPort()
	if err != nil {
		log.Printf("❌ Uygun RTP portu bulunamadı: %v", err)
		return c.Status(fiber.StatusInternalServerError).JSON(fiber.Map{"error": "Could not allocate RTP port"})
	}

	rtpSessions.Lock()
	rtpSessions.m[port] = &RtpSession{Socket: conn}
	rtpSessions.Unlock()

	// Arka planda bu portu dinlemeye başla
	go listenRtp(port, conn)

	return c.JSON(fiber.Map{
		"host": PUBLIC_IP,
		"port": port,
	})
}

type PlayAudioRequest struct {
	RtpPort         int    `json:"rtp_port"`
	AudioDataBase64 string `json:"audio_data_base64"`
}

func handlePlayAudio(c *fiber.Ctx) error {
	req := new(PlayAudioRequest)
	if err := c.BodyParser(req); err != nil {
		return c.Status(fiber.StatusBadRequest).JSON(fiber.Map{"error": "Invalid request body"})
	}

	log.Printf("[Media Service - GO] %d portundan ses çalma isteği alındı.", req.RtpPort)

	rtpSessions.RLock()
	session, ok := rtpSessions.m[req.RtpPort]
	rtpSessions.RUnlock()

	if !ok {
		return c.Status(fiber.StatusNotFound).JSON(fiber.Map{"error": "RTP session not found for this port"})
	}

	// Sesi çalmak için ayrı bir goroutine (thread) başlat
	go playAudioWithFfmpeg(session, req.AudioDataBase64)

	return c.Status(fiber.StatusAccepted).JSON(fiber.Map{"status": "Ses çalma işlemi başlatıldı."})
}

// --- Yardımcı Fonksiyonlar ---
func allocateRtpPort() (int, *net.UDPConn, error) {
	for i := 0; i < 100; i++ {
		port := rand.Intn(RTP_PORT_MAX-RTP_PORT_MIN+1) + RTP_PORT_MIN
		if port%2 != 0 {
			continue // Sadece çift portları kullan
		}

		rtpSessions.RLock()
		_, exists := rtpSessions.m[port]
		rtpSessions.RUnlock()

		if !exists {
			addr, err := net.ResolveUDPAddr("udp", fmt.Sprintf(":%d", port))
			if err != nil {
				continue
			}
			conn, err := net.ListenUDP("udp", addr)
			if err == nil {
				log.Printf("[Media Service - GO] Yeni RTP portu ayrıldı: %d", port)
				return port, conn, nil
			}
		}
	}
	return 0, nil, fmt.Errorf("100 deneme sonunda uygun port bulunamadı")
}

func listenRtp(port int, conn *net.UDPConn) {
	buffer := make([]byte, 2048)
	for {
		n, remoteAddr, err := conn.ReadFromUDP(buffer)
		if err != nil {
			log.Printf("❌ [RTP:%d] Port dinleme hatası: %v", port, err)
			return
		}

		rtpSessions.Lock()
		session, ok := rtpSessions.m[port]
		if ok && session.RemoteInfo == nil {
			log.Printf("[RTP:%d] Karşı tarafın adresi öğrenildi: %s", port, remoteAddr.String())
			session.RemoteInfo = remoteAddr
		}
		rtpSessions.Unlock()

		log.Printf("[RTP:%d] Medya paketi alındı from %s - Boyut: %d bytes", port, remoteAddr, n)
	}
}

func playAudioWithFfmpeg(session *RtpSession, audioBase64 string) {
	// Karşı taraf adresi öğrenilene kadar bekle (maksimum 5 saniye)
	for i := 0; i < 10 && session.RemoteInfo == nil; i++ {
		time.Sleep(500 * time.Millisecond)
	}
	if session.RemoteInfo == nil {
		log.Printf("❌ Zaman aşımı: Karşı taraftan RTP paketi alınamadı. Ses çalma iptal edildi.")
		return
	}

	// Base64 veriyi geçici bir dosyaya yaz
	audioData, err := base64.StdEncoding.DecodeString(audioBase64)
	if err != nil {
		log.Printf("❌ Base64 decode hatası: %v", err)
		return
	}

	tmpFile := filepath.Join(os.TempDir(), fmt.Sprintf("audio_%s.mp3", uuid.New().String()))
	if err := os.WriteFile(tmpFile, audioData, 0644); err != nil {
		log.Printf("❌ Geçici dosya yazma hatası: %v", err)
		return
	}
	defer os.Remove(tmpFile) // İşlem bitince dosyayı sil

	rtpURL := fmt.Sprintf("rtp://%s:%d?pkt_size=160", session.RemoteInfo.IP.String(), session.RemoteInfo.Port)
	log.Printf("--> FFmpeg ile %s dosyası %s adresine stream edilecek.", tmpFile, rtpURL)

	cmd := exec.Command("ffmpeg", "-re", "-i", tmpFile, "-ar", "8000", "-ac", "1", "-acodec", "pcm_mulaw", "-f", "rtp", rtpURL)

	output, err := cmd.CombinedOutput()
	if err != nil {
		log.Printf("❌ FFmpeg hatası: %v\nÇıktı: %s", err, string(output))
		return
	}
	log.Printf("--> FFmpeg işlemi başarıyla tamamlandı.")
}

// Ortam değişkenlerini okumak için yardımcı fonksiyonlar
func getEnv(key, fallback string) string {
	if value, ok := os.LookupEnv(key); ok {
		return value
	}
	return fallback
}

func getEnvAsInt(key string, fallback int) int {
	if value, ok := os.LookupEnv(key); ok {
		i, err := strconv.Atoi(value)
		if err == nil {
			return i
		}
	}
	return fallback
}
