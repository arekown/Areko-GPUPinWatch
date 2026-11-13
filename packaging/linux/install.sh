#!/usr/bin/env bash
# Installs the Areko GPUPinWatch service (run with sudo from the repository root after
# `cargo build --release -p gpw-daemon`). `--entfernen` removes it again.
set -euo pipefail

BIN=/usr/local/bin/areko-gpupinwatchd
UNIT=/etc/systemd/system/areko-gpupinwatch.service
MODS=/etc/modules-load.d/areko-gpupinwatch.conf
HERE="$(cd "$(dirname "$0")" && pwd)"
SRC_BIN="${HERE}/../../target/release/areko-gpupinwatchd"

if [[ $EUID -ne 0 ]]; then
    echo "Bitte mit sudo ausführen." >&2
    exit 1
fi

if [[ "${1:-}" == "--entfernen" ]]; then
    systemctl disable --now areko-gpupinwatch.service 2>/dev/null || true
    rm -f "$UNIT" "$BIN" "$MODS"
    systemctl daemon-reload
    echo "Areko GPUPinWatch entfernt (Einstellungen in /etc/areko-gpupinwatch bleiben)."
    exit 0
fi

if [[ ! -x "$SRC_BIN" ]]; then
    echo "$SRC_BIN fehlt – zuerst: cargo build --release -p gpw-daemon" >&2
    exit 1
fi

install -m 0755 "$SRC_BIN" "$BIN"
install -m 0644 "${HERE}/areko-gpupinwatch.service" "$UNIT"
# The on-card sensor (ASUS Astral) is read through /dev/i2c-*.
echo i2c-dev > "$MODS"
modprobe i2c-dev || true
systemctl daemon-reload
systemctl enable --now areko-gpupinwatch.service
echo "Areko GPUPinWatch installiert und gestartet."
