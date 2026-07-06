#!/usr/bin/env bash
# Permite controlar el brillo DDC/CI sin root: regla udev 'uaccess' que le da
# acceso a /dev/i2c-* al usuario con sesion activa (misma regla que instala
# ddcutil). Correr una sola vez: sudo ./setup-brillo-ddc.sh
set -euo pipefail

if [ "$(id -u)" -ne 0 ]; then
    echo "Correr con sudo: sudo $0" >&2
    exit 1
fi

cat > /etc/udev/rules.d/60-screenswitcher-i2c.rules <<'EOF'
# Acceso DDC/CI (brillo de monitores) para el usuario de la sesion activa
SUBSYSTEM=="i2c-dev", TAG+="uaccess"
EOF

udevadm control --reload-rules
udevadm trigger --subsystem-match=i2c-dev --action=change

echo "OK. Si los sliders de brillo no aparecen, cerra sesion y volve a entrar (o reinicia)."
