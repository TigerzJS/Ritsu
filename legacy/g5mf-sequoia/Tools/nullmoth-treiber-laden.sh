#!/bin/bash
# Laedt das NullMoth-NVIDIA-Treiberpaket 1.13.0 und prueft die SHA-256.
#   ./Tools/nullmoth-treiber-laden.sh [Zielordner]
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
DIR="${1:-$(dirname "$HERE")/Extras/NullMoth}"
NAME=nullmoth-nvidia-1.13.0.tar.gz
URL="https://github.com/nullmoth/nvidia-macos-driver/releases/download/v1.13.0/$NAME"
SHA=994970b1bba53e3ae8d7f500ef27b25cae8bc6818abc112d35230a58e6882fdf

sha() { if command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1"; else sha256sum "$1"; fi | awk '{print $1}'; }
mkdir -p "$DIR"
if [ -f "$DIR/$NAME" ] && [ "$(sha "$DIR/$NAME")" = "$SHA" ]; then echo "$NAME ist schon vorhanden und geprueft."; exit 0; fi
echo "Lade $NAME (ca. 270 MB) ..."
curl -fL --retry 3 -o "$DIR/$NAME.part" "$URL"
GOT="$(sha "$DIR/$NAME.part")"
[ "$GOT" = "$SHA" ] || { rm -f "$DIR/$NAME.part"; echo "SHA-256 stimmt nicht ($GOT) - Datei verworfen"; exit 1; }
mv "$DIR/$NAME.part" "$DIR/$NAME"
echo "$NAME geladen, SHA-256 OK."
