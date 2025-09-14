#!/bin/bash
set -e

PROFILE=$1
MODE=$2

# Servis bağımlılık haritası
declare -A SERVICE_DEPS=(
  ["sip-gateway"]="sip-signaling"
  ["sip-signaling"]="media-service"
  ["media-service"]="user-service dialplan-service"
  ["user-service"]=""
  ["dialplan-service"]=""
  ["agent-service"]="tts-gateway media-service user-service"
)

# Config değişikliklerinden etkilenen servisler
CONFIG_SERVICES="sip-signaling media-service user-service dialplan-service agent-service tts-gateway"

echo "🔍 Değişiklik analizi (Mod: $MODE)..."

case $MODE in
  "all")
    echo "🔄 Tüm servisler yeniden başlatılıyor..."
    docker compose -p sentiric-$PROFILE -f docker-compose.$PROFILE.yml down
    docker compose -p sentiric-$PROFILE -f docker-compose.$PROFILE.yml up -d
    ;;
  "config")
    echo "🔄 Config değişikliği için hedefli restart..."
    for service in $CONFIG_SERVICES; do
      echo "🔄 $service ve bağımlılıkları restart ediliyor..."
      docker compose -p sentiric-$PROFILE -f docker-compose.$PROFILE.yml restart $service ${SERVICE_DEPS[$service]}
    done
    ;;
  *)
    echo "ℹ️ Değişiklik tespit edildi ama restart gerekmiyor"
    ;;
esac

echo "✅ İşlem tamamlandı"