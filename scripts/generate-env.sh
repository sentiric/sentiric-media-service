#!/bin/bash
set -e
set -o pipefail

# Bu script, Docker Compose için son .env dosyasını oluşturur.
# İki aşamalı bir "derleyici" gibi çalışarak hem değişkenleri çözer
# hem de okunabilirlik için kaynak dosya yorumlarını korur.

PROFILE=${1:-dev}

# --- Temel Dizinleri Tanımla ---
SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
INFRA_DIR=$(dirname "$SCRIPT_DIR")
WORKSPACE_DIR=$(dirname "$INFRA_DIR")

# --- Kaynak ve Hedef Dosyaları Tanımla ---
CONFIG_DIR="${WORKSPACE_DIR}/sentiric-config/environments"
OUTPUT_FILE="${INFRA_DIR}/.env.generated"
PROFILE_FILE="${CONFIG_DIR}/profiles/${PROFILE}.env"
TEMP_ENV_FILE=$(mktemp)

trap 'rm -f "$TEMP_ENV_FILE"' EXIT

if [ ! -f "$PROFILE_FILE" ]; then
    echo "❌ HATA: Profil dosyası bulunamadı: $PROFILE_FILE"
    exit 1
fi

echo "🔧 Yapılandırma dosyası '${OUTPUT_FILE}' oluşturuluyor (Profil: ${PROFILE})..."

# --- AŞAMA 1: Tüm .env parçalarını, başlıklarıyla birlikte, geçici bir dosyada topla ---
while IFS= read -r line || [[ -n "$line" ]]; do
    line=$(echo "$line" | tr -d '\r')
    if [[ $line == source* ]]; then
        relative_path=$(echo "$line" | cut -d' ' -f2)
        source_file="${CONFIG_DIR}/${relative_path}"
        if [ -f "$source_file" ]; then
            # DÜZELTME: Yorum başlığını ve içeriği geçici dosyaya ekle
            echo -e "\n# Included from: ${relative_path}" >> "$TEMP_ENV_FILE"
            (cat "$source_file" | tr -d '\r' | grep -vE '^\s*#' || true) >> "$TEMP_ENV_FILE"
        fi
    fi
done < "$PROFILE_FILE"

# SECRETS_FILE="${CONFIG_DIR}/external/secrets.env"
# if [ -f "$SECRETS_FILE" ]; then
#     echo -e "\n# Included from: external/secrets.env" >> "$TEMP_ENV_FILE"
#     (cat "$SECRETS_FILE" | tr -d '\r' | grep -vE '^\s*#|^\s*$' || true) >> "$TEMP_ENV_FILE"
# fi

# --- AŞAMA 2: Değişkenleri çöz ve nihai dosyayı oluştur ---

# Geçici dosyayı oku ve değişkenleri çözerek bir "çözülmüş değişken haritası" oluştur.
# Bu harita sadece DEĞİŞKEN=değer formatında olacak.
RESOLVED_VARS=$(env -i bash -c "set -a; source '$TEMP_ENV_FILE'; env" | grep -E '^[A-Z_][A-Z0_9_]*=' | grep -vE '^(_|PWD|SHLVL)=' || true)

# Şimdi, yorumları içeren orijinal geçici dosyayı oku ve değişkenleri çözülmüş olanlarla değiştir.
# Bu, yorumları ve yapıyı korurken değerleri günceller.
awk -v vars="$RESOLVED_VARS" '
BEGIN {
    # Çözülmüş değişkenleri bir diziye ata
    split(vars, arr, "\n")
    for (i in arr) {
        split(arr[i], pair, "=")
        resolved[pair[1]] = substr(arr[i], length(pair[1]) + 2)
    }
}
/^[A-Z_][A-Z0_9_]*=/ {
    # Bir değişken satırıyla karşılaşırsak
    split($0, pair, "=")
    key = pair[1]
    if (key in resolved) {
        # Eğer bu değişken çözülmüşler listesinde varsa, çözülmüş değeri yazdır
        print key "=" resolved[key]
    } else {
        # Yoksa, orijinal satırı yazdır
        print $0
    }
    next
}
{
    # Değişken olmayan (yorum, boş satır vb.) her şeyi olduğu gibi yazdır
    print $0
}' "$TEMP_ENV_FILE" > "$OUTPUT_FILE"


# --- Dinamik Değişkenleri Sona Ekle ---
{
    echo ""
    echo "# Dynamically added by Orchestrator"
    DETECTED_IP=$(hostname -I | awk '{print $1}' || echo "127.0.0.1")
    echo "DETECTED_IP=${DETECTED_IP}"
    echo "TAG=${TAG:-latest}"
    echo "CONFIG_REPO_PATH=../sentiric-config"
} >> "$OUTPUT_FILE"

echo "✅ Yapılandırma başarıyla oluşturuldu."
# Mevcut scriptin sonuna bu satırı ekleyin:
chmod +x "$INFRA_DIR/scripts/restart-services.sh"