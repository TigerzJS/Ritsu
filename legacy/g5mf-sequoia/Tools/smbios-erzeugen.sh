#!/bin/bash
# Erzeugt eine eigene Seriennummer, MLB, UUID und ROM fuer MacBookPro16,2 und traegt sie in
# EFI/OC/config.plist und Extras/config-nach-treiber.plist ein (macOS oder Linux).
#
#   ./Tools/smbios-erzeugen.sh        (einmalig)
#   ./Tools/smbios-erzeugen.sh --neu  (neue Werte erzwingen)
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"; ROOT="$(dirname "$HERE")"
MODEL="MacBookPro16,2"
FILES=("$ROOT/EFI/OC/config.plist" "$ROOT/Extras/config-nach-treiber.plist")

if ! grep -q SERIAL-PLATZHALTER "${FILES[0]}" && [ "${1:-}" != "--neu" ]; then
  echo "Die config hat schon eine Seriennummer. Mit --neu werden neue Werte erzeugt."; exit 0
fi

case "$(uname -s)" in
  Darwin) MS="$HERE/macserial/macserial" ;;
  *)      MS="$HERE/macserial/macserial.linux" ;;
esac
chmod +x "$MS" 2>/dev/null || true
LINE="$("$MS" -m "$MODEL" -g -n 1 2>/dev/null | tail -1)"
SERIAL="$(echo "$LINE" | awk -F'|' '{gsub(/ /,"",$1); print $1}')"
MLB="$(echo "$LINE" | awk -F'|' '{gsub(/ /,"",$2); print $2}')"
[ -n "$SERIAL" ] && [ -n "$MLB" ] || { echo "macserial ist fehlgeschlagen"; exit 1; }
UUID="$(uuidgen 2>/dev/null || cat /proc/sys/kernel/random/uuid)"; UUID="$(echo "$UUID" | tr a-z A-Z)"
ROMHEX="$(od -An -N6 -tx1 /dev/urandom | tr -d ' \n')"
ROMHEX="$(printf '%02x' $(( 0x${ROMHEX:0:2} & 0xFC )))${ROMHEX:2}"

for f in "${FILES[@]}"; do
  if command -v plutil >/dev/null 2>&1 && [ "$(uname -s)" = Darwin ]; then
    plutil -replace PlatformInfo.Generic.SystemSerialNumber -string "$SERIAL" "$f"
    plutil -replace PlatformInfo.Generic.MLB -string "$MLB" "$f"
    plutil -replace PlatformInfo.Generic.SystemUUID -string "$UUID" "$f"
    plutil -replace PlatformInfo.Generic.ROM -data "$(echo "$ROMHEX" | xxd -r -p | base64)" "$f"
  else
    python3 - "$f" "$SERIAL" "$MLB" "$UUID" "$ROMHEX" <<'EOF'
import plistlib, sys
f, serial, mlb, uuid, rom = sys.argv[1:]
with open(f, "rb") as fh:
    c = plistlib.load(fh)
g = c["PlatformInfo"]["Generic"]
g.update(SystemSerialNumber=serial, MLB=mlb, SystemUUID=uuid, ROM=bytes.fromhex(rom))
with open(f, "wb") as fh:
    plistlib.dump(c, fh, sort_keys=True)
EOF
  fi
done

echo "SMBIOS fuer $MODEL eingetragen:"
echo "  Seriennummer: $SERIAL"
echo "  MLB:          $MLB"
echo "  SystemUUID:   $UUID"
echo "  ROM:          $ROMHEX"
echo "Pruefe die Seriennummer auf https://checkcoverage.apple.com - sie sollte dort NICHT gefunden werden."
