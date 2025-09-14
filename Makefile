# Sentiric Orchestrator v12.3 "Refined Conductor"
# Usage: make <command> [PROFILE=dev|core|gateway|prod|free] [SERVICE=...] [DEBUG=true]

SHELL := /bin/bash
.DEFAULT_GOAL := help

# --- Otomatik Konfigürasyon ---
PROFILE ?= $(shell cat .profile.state 2>/dev/null || echo dev)
DEBUG ?= false
ENV_FILE := .env.generated

# --- Profil Bazlı Konfigürasyon ---
ifeq ($(PROFILE),core)
# 	COMPOSE_FILES := -f docker-compose.core.yml -f docker-compose.resources.core.yml
	COMPOSE_FILES := -f docker-compose.core.yml
	ENV_CONFIG_PROFILE := core
else ifeq ($(PROFILE),gateway)
# 	COMPOSE_FILES := -f docker-compose.gateway.yml -f docker-compose.resources.gateway.yml
	COMPOSE_FILES := -f docker-compose.gateway.yml
	ENV_CONFIG_PROFILE := gateway
else ifeq ($(PROFILE),prod)
# 	COMPOSE_FILES := -f docker-compose.prod.yml -f docker-compose.resources.prod.yml
	COMPOSE_FILES := -f docker-compose.prod.yml	
	ENV_CONFIG_PROFILE := prod
else ifeq ($(PROFILE),free)
# 	COMPOSE_FILES := -f docker-compose.free.yml -f docker-compose.resources.free.yml
	COMPOSE_FILES := -f docker-compose.free.yml	
	ENV_CONFIG_PROFILE := free
else # Varsayılan profil 'dev'
# 	COMPOSE_FILES := -f docker-compose.dev.yml -f docker-compose.resources.dev.yml
	COMPOSE_FILES := -f docker-compose.dev.yml
	ENV_CONFIG_PROFILE := dev
endif

# --- Docker Compose Komutları ---
COMPOSE_BASE_CMD := docker compose -p sentiric-$(PROFILE) --env-file $(ENV_FILE) $(COMPOSE_FILES)

# Debug modu kontrolü
ifeq ($(DEBUG),true)
    COMPOSE_CMD := $(COMPOSE_BASE_CMD) --verbose
    DOCKER_BUILD_FLAGS := DOCKER_BUILDKIT=0
    LOG_LEVEL := --log-level DEBUG
else
    COMPOSE_CMD := $(COMPOSE_BASE_CMD)
    DOCKER_BUILD_FLAGS := DOCKER_BUILDKIT=1
    LOG_LEVEL := 
endif

# --- Renk Kodları ---
RED := \033[0;31m
GREEN := \033[0;32m
YELLOW := \033[0;33m
BLUE := \033[0;34m
MAGENTA := \033[0;35m
CYAN := \033[0;36m
WHITE := \033[0;37m
BOLD := \033[1m
RESET := \033[0m

# --- Güvenlik Kontrolleri ---
_PROFILE_CHECK:
	@if [ "$(PROFILE)" != "dev" ] && \
	   [ "$(PROFILE)" != "core" ] && [ "$(PROFILE)" != "gateway" ] && \
	   [ "$(PROFILE)" != "prod" ] && [ "$(PROFILE)" != "free" ]; then \
		echo -e "$(RED)❌ Hata: Geçersiz profil: $(PROFILE)$(RESET)"; \
		echo -e "   Geçerli profiller: dev, core, gateway, prod, free"; \
		exit 1; \
	fi

# --- Servis Validasyonu ---
_VALIDATE_SERVICE:
ifdef SERVICE
	@echo -e "$(BLUE)🔍 Servis validasyonu: $(SERVICE)$(RESET)"
	@if ! $(COMPOSE_CMD) config --services 2>/dev/null | grep -qw $(SERVICE); then \
		echo -e "$(RED)❌ Hata: '$(SERVICE)' servisi $(PROFILE) profilinde bulunamadı$(RESET)"; \
		echo -e "   Mevcut servisler:"; \
		$(COMPOSE_CMD) config --services 2>/dev/null | sed 's/^/     - /' || echo "     (servis listesi alınamadı)"; \
		exit 1; \
	fi
	@echo -e "$(GREEN)✅ Servis doğrulandı: $(SERVICE)$(RESET)"
endif

# --- Sezgisel Komutlar ---

# YENİ HALİ: _setup_bucket'ı sondan bir önceye taşıdık
start: _profile_check _sync_config _generate_env _validate_service ## ▶️ Platformu başlatır veya günceller (dev profilinde build eder)
	@echo -e "$(MAGENTA)🎻 Orkestra hazırlanıyor... Profil: $(PROFILE), Debug: $(DEBUG)$(RESET)"
	@echo "$(PROFILE)" > .profile.state
	@if [ "$(PROFILE)" = "dev" ]; then \
		echo -e "$(YELLOW)🚀 Kaynak koddan inşa edilerek geliştirme ortamı başlatılıyor...$(RESET)"; \
		$(DOCKER_BUILD_FLAGS) $(COMPOSE_CMD) up -d --build --remove-orphans $(SERVICE); \
	else \
		echo -e "$(YELLOW)🚀 Hazır imajlar çekiliyor ve '$(PROFILE)' profili dağıtılıyor...$(RESET)"; \
		$(COMPOSE_CMD) pull $(SERVICE); \
		$(COMPOSE_CMD) up -d --remove-orphans $(SERVICE); \
	fi
# 	Servislerde setup bucket yapma
# 	$(MAKE) _setup_bucket # Servisler başladıktan SONRA _setup_bucket'ı çağır
	@echo -e "$(GREEN)✅ Platform başlatıldı. Durum kontrolü için: make status$(RESET)"

stop: _profile_check _generate_env _validate_service ## ⏹️ Platformu durdurur (verileri korur)
	@echo -e "$(YELLOW)🛑 Platform durduruluyor (veriler korunacak)... Profil: $(PROFILE)$(RESET)"
	@$(COMPOSE_CMD) stop $(SERVICE)
	@echo -e "$(GREEN)✅ Platform durduruldu$(RESET)"

down: _profile_check _generate_env _validate_service ## 🚮 Platformu durdurur ve konteynerleri siler (verileri korur)
	@echo -e "$(YELLOW)🗑️  Platform durduruluyor ve konteynerler siliniyor (veriler korunacak)... Profil: $(PROFILE)$(RESET)"
	@$(COMPOSE_CMD) down $(if $(SERVICE),--rmi local --remove-orphans,)
	@echo -e "$(GREEN)✅ Konteynerler temizlendi$(RESET)"

down-v: _profile_check _generate_env ## 💥 Platformu durdurur, konteynerleri VE veritabanı volume'lerini siler
	@echo -e "$(RED)💥 Platform tamamen durduruluyor ve volume'ler siliniyor... Profil: $(PROFILE)$(RESET)"
	@$(COMPOSE_CMD) down -v --remove-orphans
	@echo -e "$(GREEN)✅ Volume'ler dahil tam temizlik yapıldı$(RESET)"

restart: _profile_check _generate_env _validate_service ## 🔄 Servisleri yeniden başlatır (dev profilinde build ETMEZ)
	@echo -e "$(YELLOW)🔄 Platform yeniden başlatılıyor... Profil: $(PROFILE)$(RESET)"
	@$(COMPOSE_CMD) restart $(SERVICE)
	@echo -e "$(GREEN)✅ Yeniden başlatma tamamlandı$(RESET)"

build: _profile_check _generate_env _validate_service ## 🏗️ Belirtilen servisi (veya tümünü) yeniden inşa eder (sadece dev profilleri)
	@echo -e "$(YELLOW)🏗️  Servis(ler) yeniden inşa ediliyor... Profil: $(PROFILE)$(RESET)"
	@if [ "$(PROFILE)" = "dev" ]; then \
		$(DOCKER_BUILD_FLAGS) $(COMPOSE_CMD) build $(LOG_LEVEL) $(SERVICE); \
		echo -e "$(GREEN)✅ Build tamamlandı$(RESET)"; \
	else \
		echo -e "$(RED)❌ Uyarı: 'build' komutu sadece 'dev' profillerinde çalışır. Üretim profilleri için 'pull' kullanın.$(RESET)"; \
		exit 1; \
	fi

pull: _profile_check _generate_env _validate_service ## 📥 Servislerin en son imajlarını çeker (sadece üretim profilleri)
	@echo -e "$(YELLOW)📥 En son imajlar çekiliyor... Profil: $(PROFILE)$(RESET)"
	@if [ "$(PROFILE)" != "dev" ]; then \
		$(COMPOSE_CMD) pull $(SERVICE); \
		echo -e "$(GREEN)✅ Image'lar güncellendi$(RESET)"; \
	else \
		echo -e "$(RED)❌ Uyarı: 'pull' komutu sadece üretim profillerinde çalışır. Geliştirme profilleri için 'build' kullanın.$(RESET)"; \
		exit 1; \
	fi

status: _profile_check _generate_env ## 📊 Servislerin durumunu gösterir
	@echo -e "$(BLUE)📊 Platform durumu... Profil: $(PROFILE)$(RESET)"
	@$(COMPOSE_CMD) ps $(SERVICE) || (echo -e "$(RED)❌ Durum alınamadı. Servisler çalışıyor mu?$(RESET)" && exit 1)

logs: _profile_check _generate_env _validate_service ## 📜 Servislerin loglarını canlı izler
	@echo -e "$(BLUE)📜 Loglar izleniyor... Profil: $(PROFILE) $(if $(SERVICE),Servis: $(SERVICE),)$(RESET)"
	@$(COMPOSE_CMD) logs -f $(if $(SERVICE),--tail=100,) $(SERVICE)

clean: ## 🧹 Docker ortamını TAMAMEN sıfırlar (sudo gerektirir)
	@read -p "$(RED)🔥 DİKKAT: TÜM Docker verileri (konteyner, imaj, volume) silinecek. Onaylıyor musunuz? (y/N) $(RESET)" choice; \
	if [[ "$$choice" == "y" || "$$choice" == "Y" ]]; then \
		echo -e "$(YELLOW)🧹 Platform temizleniyor...$(RESET)"; \
		for profile in dev core gateway prod free; do \
			echo -e "$(YELLOW)🧹 Temizleniyor: $$profile$(RESET)"; \
			PROFILE=$$profile $(MAKE) down-v || true; \
		done; \
		sudo docker system prune -af --volumes; \
		rm -f .env.* .profile.state; \
		echo -e "$(GREEN)✅ Temizlik tamamlandı. Sistem sıfırlandı.$(RESET)"; \
	else \
		echo -e "$(RED)❌ İşlem iptal edildi.$(RESET)"; \
	fi

# --- Yeni Gelişmiş Özellikler ---

backup: _profile_check _generate_env ## 💾 Veritabanı ve önemli verileri yedekler
	@echo -e "$(YELLOW)💾 Yedekleme başlatılıyor... Profil: $(PROFILE)$(RESET)"
	@mkdir -p backups/$(PROFILE)
	@BACKUP_FILE=backups/$(PROFILE)/backup_$$(date +%Y%m%d_%H%M%S).sql; \
	if $(COMPOSE_CMD) exec -T database pg_dumpall -U postgres > $$BACKUP_FILE 2>/dev/null; then \
		echo -e "$(GREEN)✅ Yedekleme tamamlandı: $$BACKUP_FILE$(RESET)"; \
		ls -la $$BACKUP_FILE; \
	else \
		echo -e "$(RED)❌ Yedekleme başarısız. Database servisi çalışıyor mu?$(RESET)"; \
		rm -f $$BACKUP_FILE; \
		exit 1; \
	fi

restore: _profile_check _generate_env ## 🔄 Son yedeği geri yükler (DİKKAT: Veri kaybına neden olur)
	@echo -e "$(RED)⚠️  DİKKAT: Bu işlem mevcut veritabanını SİLECEK!$(RESET)"
	@read -p "Devam etmek istiyor musunuz? (y/N) " choice; \
	if [[ "$$choice" == "y" || "$$choice" == "Y" ]]; then \
		LATEST_BACKUP=$$(ls -t backups/$(PROFILE)/*.sql 2>/dev/null | head -1); \
		if [ -z "$$LATEST_BACKUP" ]; then \
			echo -e "$(RED)❌ Yedek dosyası bulunamadı: backups/$(PROFILE)/$(RESET)"; \
			exit 1; \
		fi; \
		echo -e "$(YELLOW)🔄 Geri yükleme yapılıyor: $$LATEST_BACKUP$(RESET)"; \
		if $(COMPOSE_CMD) exec -T database psql -U postgres -f - < $$LATEST_BACKUP; then \
			echo -e "$(GREEN)✅ Geri yükleme tamamlandı$(RESET)"; \
		else \
			echo -e "$(RED)❌ Geri yükleme başarısız$(RESET)"; \
			exit 1; \
		fi; \
	else \
		echo -e "$(RED)❌ İşlem iptal edildi$(RESET)"; \
	fi

health: _profile_check _generate_env ## 📈 Servis sağlık durumunu kontrol eder
	@echo -e "$(BLUE)📈 Servis sağlık durumu kontrol ediliyor...$(RESET)"
	@$(COMPOSE_CMD) ps --format "table {{.Name}}\t{{.Status}}\t{{.Ports}}" | awk 'NR==1 || /(healthy|running)/' || \
	 (echo -e "$(RED)❌ Health check başarısız$(RESET)" && exit 1)

stats: _profile_check ## 📊 Docker kaynak kullanım istatistikleri
	@echo -e "$(BLUE)📊 Sistem istatistikleri (Profil: $(PROFILE)):$(RESET)"
	@docker stats --no-stream --format "table {{.Name}}\t{{.CPUPerc}}\t{{.MemUsage}}\t{{.NetIO}}\t{{.BlockIO}}" 2>/dev/null || \
	 echo -e "$(YELLOW)ℹ️  İstatistikler alınamadı (docker daemon çalışıyor mu?)$(RESET)"

list-services: _profile_check _generate_env ## 📋 Mevcut servisleri listeler
	@echo -e "$(BLUE)📋 Mevcut servisler ($(PROFILE)):$(RESET)"
	@$(COMPOSE_CMD) config --services 2>/dev/null | sed 's/^/  - /' || \
	 echo -e "$(RED)❌ Servis listesi alınamadı$(RESET)"

# --- Dahili Yardımcı Komutlar ---
_generate_env:
	@echo -e "$(BLUE)⚙️  Environment dosyası oluşturuluyor: $(ENV_CONFIG_PROFILE)$(RESET)"
	@bash scripts/generate-env.sh $(ENV_CONFIG_PROFILE)

_sync_config:
	@echo -e "$(BLUE)🔄 Konfigürasyon senkronizasyonu...$(RESET)"
	@if [ ! -d "../sentiric-config" ]; then \
		echo -e "$(YELLOW)🛠️ Güvenli yapılandırma reposu klonlanıyor...$(RESET)"; \
		git clone git@github.com:sentiric/sentiric-config.git ../sentiric-config; \
		git clone git@github.com:sentiric/sentiric-certificates.git ../sentiric-certificates; \
		git clone git@github.com:sentiric/sentiric-assets.git ../sentiric-assets; \
	else \
		echo -e "$(YELLOW)📦 Güvenli yapılandırma reposu güncelleniyor...$(RESET)"; \
		(cd ../sentiric-config && git pull); \
		(cd ../sentiric-certificates && git pull); \
		(cd ../sentiric-assets && git pull); \
	fi

_setup_bucket:
	@if [ ! -f "$(ENV_FILE)" ]; then \
		echo -e "$(YELLOW)⚠️ .env dosyası bulunamadı, _setup_bucket adımı atlanıyor.$(RESET)"; \
	elif grep -q 'S3_PROVIDER="minio"' "$(ENV_FILE)"; then \
		echo -e "$(BLUE)📦 S3 (MinIO) bucket'ları kontrol ediliyor/oluşturuluyor...$(RESET)"; \
		echo -e "$(YELLOW)⏳ MinIO servisinin sağlıklı olması bekleniyor...$(RESET)"; \
		timeout=60; \
		while ! $(COMPOSE_CMD) ps minio 2>/dev/null | grep -q 'healthy'; do \
			sleep 2; \
			timeout=$$((timeout-2)); \
			if [ $$timeout -le 0 ]; then \
				echo -e "$(RED)❌ Hata: MinIO servisi 60 saniye içinde sağlıklı duruma geçemedi.$(RESET)"; \
				$(COMPOSE_CMD) logs minio; \
				exit 1; \
			fi; \
		done; \
		echo -e "$(GREEN)✅ MinIO servisi sağlıklı.$(RESET)"; \
		\
		S3_BUCKET_NAME=$$(grep 'S3_BUCKET_NAME=' $(ENV_FILE) | head -n 1 | cut -d'=' -f2 | tr -d '"\r'); \
		MINIO_ROOT_USER=$$(grep 'MINIO_ROOT_USER=' $(ENV_FILE) | head -n 1 | cut -d'=' -f2 | tr -d '"\r'); \
		MINIO_ROOT_PASSWORD=$$(grep 'MINIO_ROOT_PASSWORD=' $(ENV_FILE) | head -n 1 | cut -d'=' -f2 | tr -d '"\r'); \
		\
		echo "   -> Alias ayarlanıyor..."; \
		$(COMPOSE_CMD) exec -T \
			-e MINIO_ROOT_USER="$$MINIO_ROOT_USER" \
			-e MINIO_ROOT_PASSWORD="$$MINIO_ROOT_PASSWORD" \
			minio sh -c 'mc alias set local http://localhost:9000 "$$MINIO_ROOT_USER" "$$MINIO_ROOT_PASSWORD"' || true; \
		\
		echo "   -> Bucket oluşturuluyor: $$S3_BUCKET_NAME..."; \
		$(COMPOSE_CMD) exec -T -e BUCKET="$$S3_BUCKET_NAME" minio sh -c 'mc mb "local/$$BUCKET" --ignore-existing' || true; \
		$(COMPOSE_CMD) exec -T -e BUCKET="$$S3_BUCKET_NAME" minio sh -c 'mc anonymous set public "local/$$BUCKET"' || true; \
		\
		echo -e "$(GREEN)✅ Bucket kurulum adımları tamamlandı.$(RESET)"; \
	else \
		echo -e "$(CYAN)ℹ️  S3 provider 'minio' değil. Bucket oluşturma adımı atlanıyor.$(RESET)"; \
	fi


update: _profile_check _generate_env _validate_service ## 📥 Servis imajını günceller ve yeniden başlatır
	@echo -e "$(CYAN)🚀 Servis güncelleniyor: $(SERVICE)... Profil: $(PROFILE)$(RESET)"
	@$(COMPOSE_CMD) pull $(SERVICE)
	@$(COMPOSE_CMD) restart $(SERVICE)
	@echo -e "$(GREEN)✅ Servis güncellendi: $(SERVICE)$(RESET)"	

help: ## ℹ️ Bu yardım menüsünü gösterir
	@echo ""
	@echo -e "  $(BOLD)Sentiric Orchestrator v12.3 \"Refined Conductor\"$(RESET)"
	@echo -e "  -------------------------------------------------"
	@echo -e "  Kullanım: $(CYAN)make <command> [PROFILE=dev|core|gateway|prod|free] [SERVICE=...] [DEBUG=true]$(RESET)"
	@echo -e "  Mevcut Profil: $(YELLOW)$(PROFILE)$(RESET)"
	@echo -e "  Debug Modu: $(YELLOW)$(DEBUG)$(RESET)"
	@echo ""
	@echo -e "  $(BOLD)Komutlar:$(RESET)"
	@awk 'BEGIN {FS = ":.*?## "}; /^[a-zA-Z_-]+:.*?## / {printf "    $(CYAN)%-14s$(RESET) %s\n", $$1, $$2}' $(MAKEFILE_LIST) | sort
	@echo ""
	@echo -e "  $(BOLD)Örnekler:$(RESET)"
	@echo -e "    $(GREEN)make start PROFILE=core$(RESET)          # Çekirdek servisleri başlatır"
	@echo -e "    $(GREEN)make build SERVICE=user-service$(RESET)   # user-service'i yeniden derler"
	@echo -e "    $(GREEN)make logs DEBUG=true$(RESET)             # Detaylı logları gösterir"
	@echo -e "    $(GREEN)make backup PROFILE=prod$(RESET)         # Production veritabanını yedekler"
	@echo -e "    $(GREEN)make health$(RESET)                      # Servis sağlık durumunu kontrol eder"
	@echo -e "    $(GREEN)make list-services$(RESET)               # Mevcut servisleri listeler"
	@echo ""
	@echo -e "  $(BOLD)Mevcut Servisler ($(PROFILE)):$(RESET)"
	@$(COMPOSE_CMD) config --services 2>/dev/null | sed 's/^/    - /' || echo -e "    $(YELLOW)(servis listesi alınamadı)$(RESET)"
	@echo ""

.PHONY: start stop down down-v restart build pull status logs clean help \
        backup restore health stats list-services \
        _generate_env _sync_config _profile_check _validate_service \
		update