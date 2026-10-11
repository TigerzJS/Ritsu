# Erzeugt eine eigene Seriennummer, MLB, UUID und ROM fuer MacBookPro16,2 und traegt sie in
# EFI\OC\config.plist und Extras\config-nach-treiber.plist ein.
#
#   Rechtsklick > "Mit PowerShell ausfuehren"   oder
#   powershell -ExecutionPolicy Bypass -File Tools\smbios-erzeugen.ps1
#
# Einmal ausfuehren und die Werte behalten: neue Werte bei jedem Lauf machen iMessage/FaceTime-Anmeldungen ungueltig.
$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
$Model = 'MacBookPro16,2'
$Files = @("$Root\EFI\OC\config.plist", "$Root\Extras\config-nach-treiber.plist")

$cfg = Get-Content -Raw $Files[0]
if ($cfg -notmatch 'SERIAL-PLATZHALTER' -and -not ($args -contains '-Neu')) {
    Write-Host 'Die config hat schon eine Seriennummer. Mit -Neu werden neue Werte erzeugt.' -ForegroundColor Yellow
    exit 0
}

$out = & "$PSScriptRoot\macserial\macserial.exe" -m $Model -g -n 1
if ($LASTEXITCODE -ne 0 -or -not $out) { throw 'macserial ist fehlgeschlagen' }
$parts = ($out | Select-Object -Last 1) -split '\|'
$Serial = $parts[0].Trim(); $Mlb = $parts[1].Trim()
$Uuid = [guid]::NewGuid().ToString().ToUpper()
$rom = New-Object byte[] 6
[System.Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($rom)
$rom[0] = $rom[0] -band 0xFC   # unicast, global vergebene MAC-Form
$RomB64 = [Convert]::ToBase64String($rom)

foreach ($f in $Files) {
    $s = Get-Content -Raw $f
    $s = [regex]::Replace($s, '(<key>SystemSerialNumber</key>\s*<string>)[^<]*(</string>)', "`${1}$Serial`${2}")
    $s = [regex]::Replace($s, '(<key>MLB</key>\s*<string>)[^<]*(</string>)', "`${1}$Mlb`${2}")
    $s = [regex]::Replace($s, '(<key>SystemUUID</key>\s*<string>)[^<]*(</string>)', "`${1}$Uuid`${2}")
    $s = [regex]::Replace($s, '(<key>ROM</key>\s*<data>)[^<]*(</data>)', "`${1}$RomB64`${2}")
    [IO.File]::WriteAllText($f, $s, (New-Object Text.UTF8Encoding $false))
}

Write-Host "SMBIOS fuer $Model eingetragen:" -ForegroundColor Green
Write-Host "  Seriennummer: $Serial"
Write-Host "  MLB:          $Mlb"
Write-Host "  SystemUUID:   $Uuid"
Write-Host "  ROM (Base64): $RomB64"
Write-Host 'Pruefe die Seriennummer auf https://checkcoverage.apple.com - sie sollte dort NICHT gefunden werden.'
