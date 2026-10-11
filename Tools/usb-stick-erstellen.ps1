# Befuellt einen FAT32-USB-Stick mit allem, was der Gigabyte G5 MF fuer macOS 15 Sequoia braucht:
#   X:\EFI                         OpenCore fuer den G5 MF (aus diesem Repo)
#   X:\com.apple.recovery.boot     macOS-Sequoia-Recovery direkt von Apple (macrecovery.py, braucht Python 3)
#   X:\NullMoth                    NVIDIA-Treiberpaket 1.13.0 (SHA-256 geprueft) + 1401-Mac-App
#
#   powershell -ExecutionPolicy Bypass -File Tools\usb-stick-erstellen.ps1 E:
#
# Den Stick vorher selbst formatieren: GPT, FAT32 (z. B. mit Rufus "Nicht startfaehig" oder der Datentraegerverwaltung).
# Das Skript formatiert NICHTS und schreibt nur auf den angegebenen Laufwerksbuchstaben.
param([Parameter(Mandatory = $true)][string]$Drive)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$Root = Split-Path -Parent $PSScriptRoot

$Drive = $Drive.TrimEnd('\').TrimEnd(':') + ':'
$vol = Get-Volume -DriveLetter $Drive[0]
if ($vol.FileSystem -ne 'FAT32') { throw "$Drive ist $($vol.FileSystem), nicht FAT32. Bitte als FAT32 formatieren." }
if ($vol.DriveType -ne 'Removable' -and $vol.DriveType -ne 'Fixed') { throw "$Drive ist kein Datentraeger" }
if ($Drive -ieq $env:SystemDrive) { throw "$Drive ist das Windows-Laufwerk" }

# 1) SMBIOS muss erzeugt sein
if ((Get-Content -Raw "$Root\EFI\OC\config.plist") -match 'SERIAL-PLATZHALTER') {
    Write-Host 'Erzeuge zuerst die SMBIOS-Daten ...'
    & "$PSScriptRoot\smbios-erzeugen.ps1"
}

# 2) EFI kopieren
Write-Host "Kopiere EFI nach $Drive\EFI ..."
if (Test-Path "$Drive\EFI") {
    $bak = "$Drive\EFI-alt-$(Get-Date -Format yyyyMMdd-HHmmss)"
    Write-Host "  vorhandenes EFI wird nach $bak verschoben"
    Move-Item "$Drive\EFI" $bak
}
Copy-Item -Recurse "$Root\EFI" "$Drive\EFI"

# 3) macOS-Recovery laden
$py = $null
foreach ($c in @('py', 'python', 'python3')) {
    if (Get-Command $c -ErrorAction SilentlyContinue) {
        $v = & $c -c 'import sys; print(sys.version_info[0])' 2>$null
        if ($v -eq '3') { $py = $c; break }
    }
}
$rec = "$Drive\com.apple.recovery.boot"
if (-not $py) {
    Write-Host 'Python 3 fehlt: Recovery wird NICHT geladen. Installiere Python (Microsoft Store: "Python 3.12")' -ForegroundColor Yellow
    Write-Host 'und starte das Skript erneut, oder lade die Recovery spaeter mit Tools\macrecovery.' -ForegroundColor Yellow
} elseif (Test-Path "$rec\BaseSystem.dmg") {
    Write-Host 'Recovery ist schon auf dem Stick.'
} else {
    Write-Host 'Lade macOS-15-Sequoia-Recovery von Apple (ca. 800 MB) ...'
    New-Item -ItemType Directory -Force $rec | Out-Null
    & $py "$PSScriptRoot\macrecovery\macrecovery.py" -b Mac-7BA5B2D9E42DDD94 -m 00000000000000000 download -o $rec
    if ($LASTEXITCODE -ne 0) { throw 'macrecovery ist fehlgeschlagen (Virenscanner-Webschutz/VPN pruefen)' }
}

# 4) NullMoth-Treiber + 1401-Mac-App
$nm = "$Drive\NullMoth"
New-Item -ItemType Directory -Force $nm | Out-Null
& "$PSScriptRoot\nullmoth-treiber-laden.ps1" -Ziel $nm
Copy-Item "$Root\Extras\NullMoth\1401-Mac-1.13.0.zip" $nm -Force
Copy-Item "$Root\Extras\config-nach-treiber.plist" $nm -Force

Write-Host ''
Write-Host "Fertig. Der Stick $Drive ist bereit." -ForegroundColor Green
Write-Host 'Weiter mit Schritt 3 der Anleitung (BIOS) in README.md.'
