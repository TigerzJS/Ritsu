#!/bin/bash
# Befuellt einen FAT32-USB-Stick (macOS oder Linux) mit EFI, macOS-15-Recovery und dem NullMoth-Treiber.
#   ./Tools/usb-stick-erstellen.sh /Volumes/STICK      (macOS)
#   ./Tools/usb-stick-erstellen.sh /media/$USER/STICK  (Linux)
# Den Stick vorher selbst als GPT + FAT32 (MS-DOS) formatieren. Das Skript formatiert nichts.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"; ROOT="$(dirname "$HERE")"
DST="${1:?Pfad zum gemounteten Stick angeben}"
[ -d "$DST" ] || { echo "$DST gibt es nicht"; exit 1; }
[ "$DST" != "/" ] || { echo "Nicht / angeben"; exit 1; }

grep -q SERIAL-PLATZHALTER "$ROOT/EFI/OC/config.plist" && "$HERE/smbios-erzeugen.sh"

echo "Kopiere EFI nach $DST/EFI ..."
if [ -e "$DST/EFI" ]; then
  BAK="$DST/EFI-alt-$(date +%Y%m%d-%H%M%S)"; echo "  vorhandenes EFI wird nach $BAK verschoben"; mv "$DST/EFI" "$BAK"
fi
cp -R "$ROOT/EFI" "$DST/EFI"

REC="$DST/com.apple.recovery.boot"
if [ -f "$REC/BaseSystem.dmg" ]; then echo "Recovery ist schon auf dem Stick."
elif command -v python3 >/dev/null 2>&1; then
  echo "Lade macOS-15-Sequoia-Recovery von Apple (ca. 800 MB) ..."
  mkdir -p "$REC"
  python3 "$HERE/macrecovery/macrecovery.py" -b Mac-7BA5B2D9E42DDD94 -m 00000000000000000 download -o "$REC"
else
  echo "python3 fehlt: Recovery wird NICHT geladen."
fi

NM="$DST/NullMoth"; mkdir -p "$NM"
"$HERE/nullmoth-treiber-laden.sh" "$NM"
cp "$ROOT/Extras/NullMoth/1401-Mac-1.13.0.zip" "$ROOT/Extras/config-nach-treiber.plist" "$NM/"
sync
echo "Fertig. Weiter mit Schritt 3 der Anleitung (BIOS) in README.md."
