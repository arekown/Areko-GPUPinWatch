#!/usr/bin/env bash
# Installs Areko GPUPinWatch on Linux: the service (root, systemd) and the window app with menu entry.
# Run with sudo, either from an unpacked release (binaries next to this script) or from the repository
# after `cargo build --release`. `--entfernen` removes everything again (settings stay).
set -euo pipefail

BIN=/usr/local/bin/areko-gpupinwatchd
UNIT=/etc/systemd/system/areko-gpupinwatch.service
MODS=/etc/modules-load.d/areko-gpupinwatch.conf
HERE="$(cd "$(dirname "$0")" && pwd)"
APP=/usr/local/bin/areko-gpupinwatch
DESKTOP=/usr/share/applications/areko-gpupinwatch.desktop
ICON=/usr/share/icons/hicolor/256x256/apps/areko-gpupinwatch.png
# release archive: binaries next to the script; repository: target/release
if [[ -x "${HERE}/areko-gpupinwatchd" ]]; then SRC="${HERE}"; else SRC="${HERE}/../../target/release"; fi
SRC_BIN="${SRC}/areko-gpupinwatchd"
SRC_APP="${SRC}/areko-gpupinwatch"
SRC_ICON="${HERE}/areko-gpupinwatch.png"
[[ -f "$SRC_ICON" ]] || SRC_ICON="${HERE}/../../assets/icon/icon-256.png"

if [[ $EUID -ne 0 ]]; then
    echo "Bitte mit sudo ausführen." >&2
    exit 1
fi

if [[ "${1:-}" == "--entfernen" ]]; then
    systemctl disable --now areko-gpupinwatch.service 2>/dev/null || true
    rm -f "$UNIT" "$BIN" "$MODS" "$APP" "$DESKTOP" "$ICON"
    systemctl daemon-reload
    echo "Areko GPUPinWatch entfernt (Einstellungen in /etc/areko-gpupinwatch bleiben)."
    exit 0
fi

if [[ ! -x "$SRC_BIN" ]]; then
    echo "$SRC_BIN fehlt – zuerst: cargo build --release" >&2
    exit 1
fi

install -m 0755 "$SRC_BIN" "$BIN"
install -m 0644 "${HERE}/areko-gpupinwatch.service" "$UNIT"
if [[ -x "$SRC_APP" ]]; then
    install -m 0755 "$SRC_APP" "$APP"
    install -D -m 0644 "$SRC_ICON" "$ICON"
    cat > "$DESKTOP" <<'EOD'
[Desktop Entry]
Type=Application
Name=Areko GPUPinWatch
Comment=12V-2x6 pin current guard for RTX 5090
Exec=/usr/local/bin/areko-gpupinwatch
Icon=areko-gpupinwatch
Categories=System;Monitor;
EOD
    gtk-update-icon-cache -q /usr/share/icons/hicolor 2>/dev/null || true
fi
# The on-card sensor (ASUS Astral) is read through /dev/i2c-*.
echo i2c-dev > "$MODS"
modprobe i2c-dev || true
systemctl daemon-reload
systemctl enable --now areko-gpupinwatch.service
echo "Areko GPUPinWatch installiert: Dienst läuft, Fenster im Startmenü (areko-gpupinwatch)."
