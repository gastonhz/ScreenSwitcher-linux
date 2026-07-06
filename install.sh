#!/usr/bin/env bash
# Compila e instala el applet para el usuario actual (sin sudo).
# Uso: ./install.sh
set -euo pipefail
cd "$(dirname "$0")"

[ -f "$HOME/.cargo/env" ] && source "$HOME/.cargo/env"

NAME=cosmic-applet-screen-switcher
APPID=dev.gaston.CosmicScreenSwitcher

cargo build --release

install -Dm0755 "target/release/$NAME" "$HOME/.local/bin/$NAME"
mkdir -p "$HOME/.local/share/applications" "$HOME/.config/screen-switcher"

# Exec con ruta absoluta para que cosmic-panel lo encuentre sin depender del PATH.
sed "s|^Exec=.*|Exec=$HOME/.local/bin/$NAME|" "res/$APPID.desktop" \
    > "$HOME/.local/share/applications/$APPID.desktop"

# No pisar perfiles editados a mano.
if [ ! -f "$HOME/.config/screen-switcher/profiles.kdl" ]; then
    cp profiles.kdl "$HOME/.config/screen-switcher/profiles.kdl"
fi

echo
echo "Listo. Para agregarlo al panel:"
echo "  Configuracion -> Escritorio -> Panel -> Configurar applets -> agregar 'Screen Switcher'"
echo
echo "Para el brillo DDC/CI sin root, correr una vez: sudo ./setup-brillo-ddc.sh"
